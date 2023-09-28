/*!
# Rip Rip Hooray: Rip Data.
*/

use cdtoc::{
	Toc,
	Track,
};
use crate::{
	BYTES_PER_SAMPLE,
	CacheWriter,
	RipOptions,
	RipRipError,
	RipSample,
	SAMPLE_OVERREAD,
	SAMPLES_PER_SECTOR,
	state_path,
	track_path,
	WAVE_SPEC,
};
use dactyl::traits::SaturatingFrom;
use hound::WavWriter;
use std::{
	fs::File,
	io::{
		BufReader,
		BufWriter,
	},
	ops::Range,
	path::PathBuf,
};
use super::{
	DeSerialize,
	OffsetRipIter,
	TrackQuality,
};



#[derive(Debug, Clone, Eq, PartialEq)]
/// # The State Data.
///
/// Because optical drives cannot be trusted to accurately account for the data
/// they return, we need to keep track of all uncertain data given us. With the
/// extra context, we can (hopefully) determine which sample is most likely for
/// each position.
///
/// (Known bad samples and samples confirmed via AccurateRip or CTDB don't
/// require multiple copies. Bad ones will get replaced by better data if it
/// arrives, while confirmed ones are good forever.)
///
/// The data — the rip range — is padded by 10 sectors on either side of the
/// track to account for possible drive read offsets. Depending on the offset,
/// some of that padding might not be written to, but the track itself will
/// always be covered.
///
/// This structure gets saved to disk _en masse_ in a zstd-compressed binary
/// format after each rip pass so operations can be resumed at a later date.
pub(crate) struct RipState {
	pub(super) toc: Toc,
	pub(super) track: Track,
	pub(super) rip_rng: Range<i32>,
	pub(super) data: Vec<RipSample>,
	pub(super) new: bool,
}

impl RipState {
	/// # New.
	///
	/// Resume or initialize a new data collection for the given track.
	///
	/// This method also tests out all of the different integer type
	/// conversions we'll need to use so that elsewhere we can safely
	/// lazy-cast.
	///
	/// ## Errors
	///
	/// This will return an error if the numbers can't fit in the necessary
	/// integer types, the cache is invalid, or the cache is corrupt and the
	/// user opts not to start over.
	pub(crate) fn from_track(toc: &Toc, track: Track, opts: &RipOptions)
	-> Result<Self, RipRipError> {
		// Should we pick up where we left off?
		if opts.resume() {
			match Self::from_file(toc, track, opts.reset()) {
				Ok(None) => {},
				Ok(Some(out)) => return Ok(out),
				Err(e) => return Err(e),
			}
		}

		// Pad the LSN range by 10 sectors on either end and convert to
		// samples.
		let rip_rng = track_rng_to_rip_range(track)
			.ok_or(RipRipError::RipOverflow)?;

		// The total length we might be ripping.
		let len = usize::try_from(rip_rng.end - rip_rng.start)
			.map_err(|_| RipRipError::RipOverflow)?;

		// We should also make sure the rip range in bytes fits i32, u32, and
		// usize. By testing for all three now, we can lazy-cast elsewhere.
		(rip_rng.end - rip_rng.start).checked_mul(i32::from(BYTES_PER_SAMPLE))
			.and_then(|n| u32::try_from(n).ok())
			.and_then(|n| usize::try_from(n).ok())
			.ok_or(RipRipError::RipOverflow)?;

		// The leadout needs to fit i32 in various places, so let's check for
		// that now too.
		let mut leadin = i32::try_from(toc.audio_leadin_normalized()).ok()
			.ok_or(RipRipError::RipOverflow)?;
		let mut leadout = i32::try_from(toc.audio_leadout_normalized()).ok()
			.and_then(|n| n.checked_mul(i32::from(SAMPLES_PER_SECTOR)))
			.ok_or(RipRipError::RipOverflow)?;

		// Adjust the leadin/out for the read offset.
		let offset = opts.offset();
		if track.position().is_first() && offset.is_negative() {
			leadin = leadin.checked_add(i32::from(offset.samples_abs()))
				.ok_or(RipRipError::RipOverflow)?;
		}
		else if track.position().is_last() && ! offset.is_negative() {
			leadout = leadout.checked_sub(i32::from(offset.samples_abs()))
				.ok_or(RipRipError::RipOverflow)?;
		}

		// Get the data started.
		let mut data = Vec::new();
		data.try_reserve_exact(len).map_err(|_| RipRipError::RipOverflow)?;
		for v in rip_rng.clone() {
			if v < leadin || leadout <= v { data.push(RipSample::Lead); }
			else { data.push(RipSample::Tbd); }
		}

		// Initialize without data!
		Ok(Self {
			toc: toc.clone(),
			track,
			rip_rng,
			data,
			new: true,
		})
	}

	/// # From File.
	///
	/// Read, decompress, and deserialize the cached state, if any.
	///
	/// If there is no cached state, `None` will be returned.
	///
	/// ## Errors
	///
	/// This will return an error if the cache location cannot be determined,
	/// the cache exists and cannot be deserialized, or the data is in someway
	/// nonsensical.
	pub(crate) fn from_file(toc: &Toc, track: Track, reset: bool)
	-> Result<Option<Self>, RipRipError> {
		let src = state_path(toc, track)?;
		if let Ok(file) = File::open(src) {
			// Read -> decompress -> deserialize.
			let mut out: Self = zstd::stream::Decoder::new(file).ok()
				.and_then(|dec| {
					let mut dec = BufReader::new(dec);
					Self::deserialize_from(&mut dec)
				})
				.ok_or_else(|| RipRipError::StateCorrupt(track.number()))?;

			// Return the instance if it matches the info we're expecting.
			if out.toc.eq(toc) && out.track == track {
				// Reset the counts?
				if reset {
					out.reset();
					let _res = out.save_state();
				}
				Ok(Some(out))
			}
			else {
				Err(RipRipError::StateCorrupt(track.number()))
			}
		}
		else { Ok(None) }
	}
}

impl RipState {
	/// # Reset Counts.
	///
	/// Drop all maybe counts to one so their sectors can be reread.
	pub(crate) fn reset(&mut self) {
		for v in &mut self.data {
			if let RipSample::Maybe(v) = v {
				v.reset();
			}
		}
	}

	/// # Save State.
	///
	/// Save a copy of the state to disk so the rip can be resumed at some
	/// future date.
	///
	/// To help mitigate the storage requirements, the serialized data is
	/// compressed with default-level zstd.
	///
	/// ## Errors
	///
	/// This will bubble up any errors encountered along the way.
	pub(crate) fn save_state(&self) -> Result<(), RipRipError> {
		// The destination path.
		let dst = state_path(&self.toc, self.track)
			.map_err(|_| RipRipError::StateSave(self.track.number()))?;

		// Serialize -> compress -> write to tmpfile.
		let mut writer = CacheWriter::new(&dst)?;
		zstd::stream::Encoder::new(writer.writer(), 0).ok()
			.and_then(|mut enc| {
				// Enable long distance matching.
				let _res = enc.long_distance_matching(true);

				// Let zstd know how much data to expect.
				let _res = u64::try_from(self.serialized_len())
					.ok()
					.and_then(|n| enc.set_pledged_src_size(Some(n)).ok())
        			.and_then(|_| enc.include_contentsize(true).ok());

				// Parallelize the encoding if possible.
				let _res = std::thread::available_parallelism().ok()
					.and_then(|n| u32::try_from(n.get()).ok())
					.and_then(|par| enc.multithread(par).ok());

				// Push the compressor into a BufWriter to make the chunking
				// more efficient. Both writers flush on drop.
				let mut enc = BufWriter::with_capacity(
					zstd::stream::Encoder::<File>::recommended_input_size(),
					enc.auto_finish(),
				);
				self.serialize_into(&mut enc)
			})
			.ok_or_else(|| RipRipError::StateSave(self.track.number()))?;

		// Save the tmpfile to dst.
		writer.finish()
	}

	/// # Save Track.
	///
	/// Write the best-available copy of the track to WAV format, and return
	/// the path for reference.
	///
	/// ## Errors
	///
	/// This will bubble up any I/O-related errors encountered, but should be
	/// fine.
	pub(crate) fn save_track(&self) -> Result<PathBuf, RipRipError> {
		let dst = track_path(&self.toc, self.track)?;
		let samples = self.track_slice();
		let mut writer = CacheWriter::new(&dst)?;
		let mut wav = WavWriter::new(writer.writer(), WAVE_SPEC)
			.map_err(|_| RipRipError::Write(dst.to_string_lossy().into_owned()))?;

		// In CD contexts, a sample is general one L+R pair. In other
		// contexts, like hound, L and R are each their own sample. (We
		// need to double our internal count to match.)
		{
			let mut wav_writer = wav.get_i16_writer(u32::saturating_from(samples.len()) * 2);
			for sample in samples {
				let sample = sample.as_array();
				wav_writer.write_sample(i16::from_le_bytes([sample[0], sample[1]]));
				wav_writer.write_sample(i16::from_le_bytes([sample[2], sample[3]]));
			}
			wav_writer.flush().map_err(|_| RipRipError::Write(dst.to_string_lossy().into_owned()))?;
		}

		// Finish up the wav.
		wav.flush().ok()
			.and_then(|_| wav.finalize().ok())
			.ok_or_else(|| RipRipError::Write(dst.to_string_lossy().into_owned()))?;

		// Save the file.
		writer.finish().map(|_| dst)
	}
}

impl RipState {
	/// # Inner Track Range.
	///
	/// Return the range of `self.data` representing the actual track, i.e.
	/// minus the padding samples.
	fn inner_index_track_rng(&self) -> Range<usize> {
		let start = usize::from(SAMPLE_OVERREAD);
		let end = self.data.len() - usize::from(SAMPLE_OVERREAD);
		start..end
	}
}

impl RipState {
	/// # Offset Rip Iterator.
	///
	/// Return an offset-aware iterator of the sector LSNs to read from, and
	/// the mutable slices to write the responses back to.
	///
	/// ## Errors.
	///
	/// This will return an error if there's a bug in the programming, but that
	/// shouldn't happen. ;)
	pub(super) fn offset_rip_iter(&mut self, opts: &RipOptions)
	-> Result<OffsetRipIter, RipRipError> {
		// Let's start with the read parts.
		let sector_range = self.sector_rip_range();
		let mut lsn_start = sector_range.start;
		let mut lsn_end = sector_range.end;
		let offset = opts.offset();
		let sectors_abs = i32::from(offset.sectors_abs());

		// Negative offsets require the data be pushed forward to "start" at
		// the right place, so we can't read the very end.
		if offset.is_negative() { lsn_end -= sectors_abs; }
		// Positive offsets require data be pulled backward instead, so we have
		// to skip the very beginning.
		else { lsn_start += sectors_abs; }

		// Now let's figure out where to slice from. Convert the start to
		// samples, subtract the offset (which may be negative), then subtract
		// the first sample in the full range to get the relative slice index.
		let idx_start = usize::try_from(
			lsn_start * i32::from(SAMPLES_PER_SECTOR)
				- i32::from(offset.samples())
				- self.rip_rng.start
		)
			.map_err(|_| RipRipError::Bug("Invalid OffsetRipIter starting index."))?;

		// The end is easier; just convert the lsn range to samples and add it
		// to our start.
		let idx_end = idx_start + (lsn_start..lsn_end).len()
			* usize::from(SAMPLES_PER_SECTOR);
		if self.data.len() < idx_end {
			return Err(RipRipError::Bug("Invalid OffsetRipIter ending index."));
		}

		OffsetRipIter::new(
			lsn_start..lsn_end,
			&mut self.data[idx_start..idx_end],
			opts.backwards(),
		)
	}

	/// # Full Rip Slice.
	///
	/// Return a slice of all of the samples gathered, not just the track bits.
	pub(crate) fn rip_slice(&self) -> &[RipSample] { &self.data }

	/// # Sector Rip Range.
	///
	/// Convert the sample rip range to a sector rip range and return it.
	pub(crate) const fn sector_rip_range(&self) -> Range<i32> {
		self.rip_rng.start.wrapping_div(SAMPLES_PER_SECTOR as i32)..
		self.rip_rng.end.wrapping_div(SAMPLES_PER_SECTOR as i32)
	}

	/// # Table of Contents.
	///
	/// Return the Table of Contents.
	pub(crate) const fn toc(&self) -> &Toc { &self.toc }

	/// # Track.
	///
	/// Return a copy of the `Track` object.
	pub(crate) const fn track(&self) -> Track { self.track }

	/// # Track Quality.
	///
	/// Add up the bad, maybe, likely, and confirmed samples within the track
	/// range.
	pub(super) fn track_quality(&self, opts: &RipOptions) -> TrackQuality {
		let slice = self.track_slice();
		TrackQuality::new(slice, opts.rereads())
	}

	/// # Track Slice.
	///
	/// Return a slice of the samples comprising the actual track, i.e. minus
	/// the padding.
	pub(crate) fn track_slice(&self) -> &[RipSample] {
		let rng = self.inner_index_track_rng();
		&self.data[rng]
	}
}

impl RipState {
	/// # Is New?
	///
	/// Returns `true` if the data was not seeded from a previous state.
	pub(crate) const fn is_new(&self) -> bool { self.new }

	/// # Quick Hash.
	///
	/// Hash the contents of the ripped data. This provides an easy metric for
	/// comparison to e.g. determine if anything changed between runs.
	pub(crate) fn quick_hash(&self) -> u32 {
		use std::hash::Hash;
		let mut hasher = crc32fast::Hasher::new();
		self.data.hash(&mut hasher);
		hasher.finalize()
	}
}



/// # Track Range to Rip Range.
pub(super) fn track_rng_to_rip_range(track: Track) -> Option<Range<i32>> {
	let rng = track.sector_range_normalized();
	let rng =
		i32::try_from(rng.start).ok()
			.and_then(|n| n.checked_mul(i32::from(SAMPLES_PER_SECTOR)))
			.and_then(|n| n.checked_sub(i32::from(SAMPLE_OVERREAD)))?..
		i32::try_from(rng.end).ok()
			.and_then(|n| n.checked_mul(i32::from(SAMPLES_PER_SECTOR)))
			.and_then(|n| n.checked_add(i32::from(SAMPLE_OVERREAD)))?;
	Some(rng)
}
