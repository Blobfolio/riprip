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
	NULL_SAMPLE,
	ReadOffset,
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
use serde::{
	de,
	Deserialize,
	ser::{
		self,
		SerializeStruct,
	},
	Serialize,
};
use std::{
	fmt,
	fs::File,
	io::{
		BufReader,
		BufWriter,
	},
	ops::Range,
	path::PathBuf,
};
use super::TrackQuality;



#[derive(Debug, Clone)]
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
	toc: Toc,
	track: Track,
	rip_rng: Range<i32>,
	data: Vec<RipSample>,
	new: bool,
}

impl<'de> Deserialize<'de> for RipState {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where D: de::Deserializer<'de> {
		const FIELDS: &[&str] = &["toc", "track", "data"];
		struct RipStateVisitor;

		impl<'de> de::Visitor<'de> for RipStateVisitor {
			type Value = RipState;

			fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
				formatter.write_str("struct RipState")
			}

			// Bincode is sequence-driven, so this is all we need.
			fn visit_seq<V>(self, mut seq: V) -> Result<Self::Value, V::Error>
            where V: de::SeqAccess<'de> {
				let toc: Toc = seq.next_element()?
					.ok_or_else(|| de::Error::invalid_length(0, &self))?;

				// The track is stored by index number only; we need to fetch
				// the corresponding object from the TOC.
				let track = seq.next_element()?
					.and_then(|n: u8|
						if n == 0 { toc.htoa() }
						else { toc.audio_track(usize::from(n)) }
					)
					.ok_or_else(|| de::Error::invalid_length(1, &self))?;

				// The rip_rng is derived from the track.
				let rip_rng = track_rng_to_rip_range(track)
					.ok_or_else(|| de::Error::invalid_length(1, &self))?;

				// The data is a straightforward vec, but we need to check its
				// length covers the full rip range.
				let data = seq.next_element()?
					.filter(|d: &Vec<RipSample>| d.len() == rip_rng.len())
					.ok_or_else(|| de::Error::invalid_length(2, &self))?;

				Ok(RipState {
					toc,
					track,
					rip_rng,
					data,
					new: false,
				})
            }
		}

		deserializer.deserialize_struct("RipState", FIELDS, RipStateVisitor)
	}
}

impl Serialize for RipState {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where S: ser::Serializer {
		let mut state = serializer.serialize_struct("RipState", 3)?;

		state.serialize_field("toc", &self.toc)?;
		state.serialize_field("track", &self.track.number())?;
		state.serialize_field("data", &self.data)?;

		state.end()
	}
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
	pub(crate) fn from_track(toc: &Toc, track: Track, resume: bool, reset_counts: bool)
	-> Result<Self, RipRipError> {
		// Should we pick up where we left off?
		if resume {
			match Self::from_file(toc, track, reset_counts) {
				Ok(None) | Err(RipRipError::StateCorrupt(_)) => {},
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
		let leadout = i32::try_from(toc.audio_leadout()).ok()
			.and_then(|n| n.checked_mul(i32::from(SAMPLES_PER_SECTOR)))
			.ok_or(RipRipError::RipOverflow)?;

		// If only there were a ::try_with_capacity()!
		let mut data = Vec::new();
		data.try_reserve(len).map_err(|_| RipRipError::RipOverflow)?;

		// Prepopulate the entries for each .
		for v in rip_rng.clone() {
			if v < 0 || leadout < v {
				data.push(RipSample::Confirmed(NULL_SAMPLE));
			}
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
	fn from_file(toc: &Toc, track: Track, reset_counts: bool)
	-> Result<Option<Self>, RipRipError> {
		let src = state_path(toc, track)?;
		if let Ok(file) = File::open(src) {
			// Read -> decompress -> deserialize.
			let mut out: Self = zstd::stream::Decoder::new(file).ok()
				.and_then(|dec| bincode::deserialize_from(BufReader::new(dec)).ok())
				.ok_or_else(|| RipRipError::StateCorrupt(track.number()))?;

			// Return the instance if it matches the info we're expecting.
			if out.toc.eq(toc) && out.track == track {
				// Reset the counts?
				if reset_counts {
					out.reset_counts();
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
	/// # Confirm Track.
	///
	/// Mark all track samples as confirmed.
	///
	/// This is called after AccurateRip and/or CUETools independently verify
	/// the data we've collected. If they're happy, we're happy.
	pub(crate) fn confirm_track(&mut self) {
		let rng = self.inner_index_track_rng();
		for v in &mut self.data[rng] {
			if ! v.is_confirmed() {
				*v = RipSample::Confirmed(v.as_array());
			}
		}
	}

	/// # Reset Counts.
	///
	/// Drop all maybe counts to one so their sectors can be reread.
	pub(crate) fn reset_counts(&mut self) {
		for v in &mut self.data {
			if let RipSample::Maybe(v) = v {
				v.reset_counts();
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
				// Try to parallelize, but don't die if it fails.
				let _res = std::thread::available_parallelism()
					.ok()
					.and_then(|n| u32::try_from(n.get()).ok())
					.and_then(|par| enc.multithread(par).ok());

				// Push the compressor into a BufWriter to make bincode's
				// chunking more efficient. Both writers flush on drop.
				bincode::serialize_into(BufWriter::new(enc.auto_finish()), self).ok()
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

	/// # Sample to Inner Index.
	///
	/// Return the index in `self.data` corresponding to a particular sample
	/// number (relative to the start of the disc).
	///
	/// If for some reason the sample is out of range, `None` is returned.
	fn sample_to_inner_index(&self, sample: i32) -> Option<usize> {
		if self.rip_rng.contains(&sample) {
			Some(usize::saturating_from(sample - self.rip_rng.start))
		}
		else { None }
	}
}

impl RipState {
	/// # Mutable Offset Sector.
	///
	/// Return the mutable data corresponding to LSN at the provided offset,
	/// or an empty slice if the result is out of range.
	pub(crate) fn offset_sector_mut(&mut self, lsn: i32, offset: ReadOffset)
	-> Result<&mut [RipSample], RipRipError> {
		let start = lsn * i32::from(SAMPLES_PER_SECTOR) - i32::from(offset.samples());
		if let Some(start) = self.sample_to_inner_index(start) {
			let end = start + usize::from(SAMPLES_PER_SECTOR);
			if end <= self.data.len() {
				return Ok(&mut self.data[start..end]);
			}
		}

		Err(RipRipError::Bug("Offset sample out of range!"))
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
	pub(super) fn track_quality(&self, rereads: (u8, u8)) -> TrackQuality {
		TrackQuality::new(self.track_slice(), rereads)
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

	/*
	/// # Is Confirmed?
	///
	/// Returns `true` if the track has been independently confirmed by
	/// AccurateRip and/or CUETools.
	///
	/// The padding data has no effect on the result.
	pub(crate) fn is_confirmed(&self) -> bool {
		self.track_slice()
			.iter()
			.all(RipSample::is_confirmed)
	}

	/// # Is Likely?
	///
	/// Returns true if all of the samples in the rippable range are likely or
	/// better.
	pub(crate) fn is_likely(&self, offset: ReadOffset, rereads: (u8, u8)) -> bool {
		// We'll be missing bits at the beginning or end depending on the
		// offset.
		let samples_abs = usize::from(offset.samples_abs());
		let len = self.data.len();
		if len < samples_abs { return false; } // Shouldn't happen!
		let slice =
			if offset.is_negative() { &self.data[samples_abs..] }
			else { &self.data[..len - samples_abs] };

		slice.iter().all(|v| v.is_likely(rereads))
	}
	*/

	/// # Quick Hash.
	///
	/// Hash the contents of the ripped data. This provides an easy metric for
	/// comparison to e.g. determine if anything changed between runs.
	pub(crate) fn quick_hash(&self) -> u64 {
		use std::hash::{BuildHasher, Hash, Hasher};
		let mut hasher = crate::AHASHER.build_hasher();
		self.data.hash(&mut hasher);
		hasher.finish()
	}
}



/// # Track Range to Rip Range.
fn track_rng_to_rip_range(track: Track) -> Option<Range<i32>> {
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
