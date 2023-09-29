/*!
# Rip Rip Hooray: Rip Data.
*/

use cdtoc::{
	Toc,
	TocKind,
	Track,
};
use crate::{
	BYTES_PER_SAMPLE,
	CacheWriter,
	ReadOffset,
	RipOptions,
	RipRipError,
	RipSample,
	SAMPLE_OVERREAD,
	SAMPLES_PER_SECTOR,
	state_path,
	track_path,
};
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



/// # Buffer Size.
///
/// The buffer size to use for `BufReader`/`BufWriter` instances.
const BUFFER_SIZE: usize = 16 * 1024;

/// # Magic Bytes.
///
/// This is used to identify `RipState` files, as well as the format "version"
/// used at the time of their construction, making sure we don't waste time
/// trying to shove bytes into the wrong format.
const MAGIC: [u8; 8] = *b"RRip0002";

/// # Wave Header.
///
/// Every header is the same, except for two four-byte blocks specifying the
/// file and data sizes.
const WAVE_HEADER: [u8; 44] = [
	82, 73, 70, 70,    // "RIFF"
	0, 0, 0, 0,        // Total file size, minus RIFF and these four bytes
	87, 65, 86, 69,    // "WAVE"
	102, 109, 116, 32, // "fmt "
	16, 0, 0, 0,       // 16: length of the above.
	1, 0,              // 1: PCM format.
	2, 0,              // 2: Number of channels.
	68, 172, 0, 0,     // 44,100: Sample rate.
	16, 177, 2, 0,     // 176,400: Sample rate * bps * channels / 8.
	4, 0,              // 4: bps * channels / 8.
	16, 0,             // 16: Bits per sample.
	100, 97, 116, 97,  // "data"
	0, 0, 0, 0,        // Size of the data portion (all that comes next).
];



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
	toc: Toc,
	track: Track,
	disc_rng: Range<i32>,
	rip_rng: Range<i32>,
	data: Vec<RipSample>,
	new: bool,
}

impl RipState {
	/// # New.
	///
	/// Begin or resume a state file for the given track, returning a new
	/// instance.
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
	pub(crate) fn new(toc: &Toc, track: Track, opts: &RipOptions)
	-> Result<Self, RipRipError> {
		let disc_rng = accessible_range(toc, opts.offset())
			.ok_or(RipRipError::RipOverflow)?;
		let mut out = Self {
			toc: toc.clone(),
			track,
			disc_rng,
			rip_rng: 0..0,
			data: Vec::new(),
			new: true,
		};
		out.init(track, opts)?;
		Ok(out)
	}

	/// # Replace (Track).
	///
	/// Same as `RipState::new`, but re-use the existing instance's allocations
	/// to reduce the memory overhead. If the track is the same as the one
	/// already represented, it is left alone.
	///
	/// ## Errors
	///
	/// This will return an error if the numbers can't fit in the necessary
	/// integer types, the cache is invalid, or the cache is corrupt and the
	/// user opts not to start over.
	pub(crate) fn replace(&mut self, track: Track, opts: &RipOptions)
	-> Result<(), RipRipError> {
		if self.track == track { Ok(()) }
		else { self.init(track, opts) }
	}

	/// # Initialize.
	///
	/// This does all the actual work for `RipState::new` and `RipState::replace`.
	///
	/// ## Errors
	///
	/// This will return an error if the numbers can't fit in the necessary
	/// integer types, the cache is invalid, or the cache is corrupt and the
	/// user opts not to start over.
	fn init(&mut self, track: Track, opts: &RipOptions) -> Result<(), RipRipError> {
		use std::io::Read;

		// Assume this is new until we learn differently.
		self.new = true;
		self.track = track;
		self.rip_rng = track_rng_to_rip_range(track).ok_or(RipRipError::RipOverflow)?;

		// Let's test the rip range as bytes in various integer sizes to make
		// sure we can freely cast last on.
		(self.rip_rng.end - self.rip_rng.start).checked_mul(i32::from(BYTES_PER_SAMPLE))
			.and_then(|n| u32::try_from(n).ok())   // For wave.
			.and_then(|n| usize::try_from(n).ok()) // For indexing.
			.and_then(|n| isize::try_from(n).ok()) // For vector capacity.
			.ok_or(RipRipError::RipOverflow)?;

		// Reset the data.
		self.data.truncate(0);
		let len = self.rip_rng.len();
		self.data.try_reserve_exact(len).map_err(|_| RipRipError::RipOverflow)?;

		// Load it from a previous session?
		if opts.resume() {
			let idx = track.number();
			let src = state_path(&self.toc, track)?;
			if let Ok(file) = File::open(src) {
				let mut file = BufReader::with_capacity(BUFFER_SIZE, file);

				// Magic header.
				let mut buf = [0u8; MAGIC.len()];
				if file.read_exact(&mut buf).is_err() || buf != MAGIC {
					return Err(RipRipError::StateCorrupt(idx));
				}

				// We'll check this after the data is read.
				let hash = u32::deserialize_from(&mut file)
					.ok_or(RipRipError::StateCorrupt(idx))?;

				// Load the data.
				for _ in 0..len {
					let v = RipSample::deserialize_from(&mut file)
						.ok_or(RipRipError::StateCorrupt(idx))?;
					self.data.push(v);
				}

				// Check the hash now to verify the toc, track, data are
				// (reasonably) what we expected.
				if hash != self.quick_hash() {
					return Err(RipRipError::StateCorrupt(idx));
				}

				// This isn't new, obviously.
				self.new = false;

				// Reset the data?
				if opts.reset() && self.reset() { self.save_state()?; }

				// We're good!
				return Ok(());
			}
		}

		// Prepopulate the data.
		for v in self.rip_rng.clone() {
			if self.disc_rng.contains(&v) { self.data.push(RipSample::Tbd); }
			else { self.data.push(RipSample::Lead); }
		}

		// Done!
		Ok(())
	}
}

impl RipState {
	/// # Reset Counts.
	///
	/// Drop all maybe counts to one so their sectors can be reread. Returns
	/// `true` if anything winds up getting changed.
	fn reset(&mut self) -> bool {
		let before = self.quick_hash();
		for v in &mut self.data {
			if let RipSample::Maybe(v) = v { v.reset(); }
		}
		before != self.quick_hash()
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
		use std::io::Write;

		// The destination path.
		let dst = state_path(&self.toc, self.track)
			.map_err(|_| RipRipError::StateSave(self.track.number()))?;

		// Serialize -> compress -> write to tmpfile.
		let mut writer = CacheWriter::new(&dst)?;
		{
			let mut buf = BufWriter::with_capacity(BUFFER_SIZE, writer.writer());
			let idx = self.track.number();

			// The first twelve bytes are reserved for some magic header bits
			// and a CRC32 hash of the toc, track, and data.
			buf.write_all(MAGIC.as_slice()).ok()
				.and_then(|_| self.quick_hash().serialize_into(&mut buf))
				.ok_or(RipRipError::StateSave(idx))?;

			// Everything else is the sample data…
			for v in &self.data {
				v.serialize_into(&mut buf).ok_or(RipRipError::StateSave(idx))?;
			}
		}
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
		use std::io::Write;

		let dst = track_path(&self.toc, self.track)?;
		let data = self.track_slice();

		// The data length is easy: four bytes per sample.
		let data_len = u32::try_from(data.len())
			.ok()
			.and_then(|n| n.checked_mul(u32::from(BYTES_PER_SAMPLE)))
			.ok_or_else(|| RipRipError::Write(dst.to_string_lossy().into_owned()))?;

		// The file length excludes "RIFF" and the four bytes specifying the
		// file length.
		let file_len = 44 - 8 + data_len;

		// Write the data!
		let mut writer = CacheWriter::new(&dst)?;
		{
			let mut buf = BufWriter::with_capacity(BUFFER_SIZE, writer.writer());

			// The header comes first; we just need to fill out the
			// size-related blocks before pushing it.
			let mut header = WAVE_HEADER;
			header[4..8].copy_from_slice(file_len.to_le_bytes().as_slice());
			header[40..].copy_from_slice(data_len.to_le_bytes().as_slice());
			buf.write_all(header.as_slice())
				.map_err(|_| RipRipError::Write(dst.to_string_lossy().into_owned()))?;

			// Now it's just straight PCM funtimes!
			for v in data {
				buf.write_all(v.as_slice())
					.map_err(|_| RipRipError::Write(dst.to_string_lossy().into_owned()))?;
			}
		}
		writer.finish()?;
		Ok(dst)
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
		self.toc.hash(&mut hasher);
		self.track.hash(&mut hasher);
		self.data.hash(&mut hasher);
		hasher.finalize()
	}
}



/// # Accessible Range.
///
/// Find the region of the disc (containing audio) that is accessible to the
/// drive, given its offset.
fn accessible_range(toc: &Toc, offset: ReadOffset) -> Option<Range<i32>> {
	// The base leadin will usually be zero, but if there's a data session
	// before the first track, we'll want to start with the actual audio.
	let mut leadin =
		if matches!(toc.kind(), TocKind::DataFirst) {
			i32::try_from(toc.audio_leadin_normalized()).ok()
				.and_then(|n| n.checked_mul(i32::from(SAMPLES_PER_SECTOR)))?
		}
		else { 0 };

	// The leadout is what it is.
	let mut leadout = i32::try_from(toc.audio_leadout_normalized()).ok()
		.and_then(|n| n.checked_mul(i32::from(SAMPLES_PER_SECTOR)))?;

	// A negative offset won't be able to reach the beginning.
	if offset.is_negative() {
		leadin = leadin.checked_add(i32::from(offset.samples_abs()))?;
	}
	// A positive offset won't be able to reach the end.
	else {
		leadout = leadout.checked_sub(i32::from(offset.samples_abs()))?;
	}

	// Can't imagine this would ever not be the case, but might as well check.
	if leadin < leadout { Some(leadin..leadout) }
	else { None }
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
