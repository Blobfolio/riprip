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
	Sample,
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
			match v {
				RipSample::Maybe((_, count)) => { *count = 1; },
				RipSample::Contentious(set) => {
					for (_, count, _) in &mut *set { *count = 1; }
				},
				_ => {},
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



#[derive(Debug, Clone, Default, Deserialize, Hash, Serialize)]
/// # Rip Sample.
///
/// This enum combines sample value(s) and their status.
pub(crate) enum RipSample {
	#[default]
	/// # Unread samples.
	Tbd,

	/// Samples that came down with C2 or read errors.
	Bad(Sample),

	/// Allegedly good sample, uncontested.
	Maybe((Sample, u8)),

	/// Like `Maybe`, but contested and/or desynched.
	Contentious(Vec<(Sample, u8, bool)>),

	/// Samples in the leadin/leadout — that we can't access and thus have to
	/// assume are null — or ones that have been independently verified by
	/// AccurateRip and/or CUETools.
	Confirmed(Sample),
}

impl RipSample {
	/// # As Array.
	///
	/// Return the most appropriate single sample 4-byte value as an array.
	pub(crate) fn as_array(&self) -> Sample {
		match self {
			Self::Tbd => NULL_SAMPLE,
			Self::Bad(s) | Self::Confirmed(s) | Self::Maybe((s, _)) => *s,
			Self::Contentious(s) => s.first().map_or(NULL_SAMPLE, |s| s.0),
		}
	}

	/// # As Slice.
	///
	/// Return the most appropriate single sample 4-byte value as a slice.
	pub(crate) fn as_slice(&self) -> &[u8] {
		match self {
			Self::Tbd => NULL_SAMPLE.as_slice(),
			Self::Bad(s) | Self::Confirmed(s) | Self::Maybe((s, _)) => s.as_slice(),
			Self::Contentious(s) => s.first().map_or_else(|| NULL_SAMPLE.as_slice(), |s| s.0.as_slice()),
		}
	}
}

impl RipSample {
	/*
	/// # Is Bad?
	pub(crate) const fn is_bad(&self) -> bool { matches!(self, Self::Bad(_)) }

	/// # Is Maybe?
	pub(crate) const fn is_maybe(&self) -> bool { matches!(self, Self::Contentious(_)) }
	*/

	/// # Is Confirmed?
	pub(crate) const fn is_confirmed(&self) -> bool { matches!(self, Self::Confirmed(_)) }

	/// # Is Likely?
	///
	/// A "maybe" is "likely" if it has been returned at least `cutoff` times
	/// and twice as much as any other competing value.
	///
	/// If this is called on `RipSample::Confirmed`, it will also return `true`.
	pub(crate) fn is_likely(&self, rereads: (u8, u8)) -> bool {
		match self {
			Self::Tbd | Self::Bad(_) => false,
			Self::Confirmed(_) => true,
			Self::Maybe((_, count)) => rereads.0 <= *count,
			Self::Contentious(set) =>
				if set.len() < 2 { false }
				else {
					rereads.0 <= set[0].1 &&
					set.iter()
						.skip(1)
						.fold(0_u8, |acc, (_, v, _)| acc.saturating_add(*v))
						.saturating_mul(rereads.1)
						.min(254) < set[0].1
				},
		}
	}
}

impl RipSample {
	/// # Update Sample.
	///
	/// See `update_bad` for what happens if there's a C2 error. Otherwise,
	/// this method changes things as follows:
	///
	/// TBD and Bad samples are simply replaced.
	///
	/// Maybe samples are incremented if the new value matches, or converted
	/// to Contentious if different.
	///
	/// Contentious values are incremented if the new value matches, or the
	/// new value is added to the end of the list. (If the only reason for
	/// contention was a sync error and that is fixed by the new read, it is
	/// changed to Maybe.)
	///
	/// Confirmed stay the same.
	pub(crate) fn update(&mut self, new: Sample, err_c2: bool, err_sync: bool) {
		// Send bad samples to a different method to halve the onslaught of
		// conditional handling. Haha.
		if err_c2 {
			return self.update_bad(new, err_sync);
		}

		match self {
			// Always update a TBD.
			Self::Tbd | Self::Bad(_) =>
				if err_sync { *self = Self::Contentious(vec![(new, 1, false)]); }
				else { *self = Self::Maybe((new, 1)); },

			// Simple Maybes.
			Self::Maybe((old, count)) =>
				// Bump non-error matches.
				if new.eq(old) { *count = count.saturating_add(1); }
				// Switch to complex type if this is a good but different
				// sample.
				else {
					*self = Self::Contentious(vec![
						(*old, *count, true),
						(new, 1, ! err_sync),
					]);
				},

			// Annoying Vector Maybes.
			Self::Contentious(set) => {
				let mut found = true;
				for old in &mut *set {
					// Bump the count and re-sort if this value was already
					// present.
					if new == old.0 {
						old.1 = old.1.saturating_add(1);
						old.2 = old.2 || ! err_sync;
						found = true;
						break;
					}
				}

				// The sort order or type might need to change.
				if found {
					// Simplify.
					if set.len() == 1 {
						if set[0].2 {
							*self = Self::Maybe((set[0].0, set[0].1));
						}
					}
					// Re-Sort.
					else {
						set.sort_unstable_by(|a, b| b.1.cmp(&a.1));
					}
				}
				// Otherwise add it new.
				else { set.push((new, 1, ! err_sync)); }
			},

			// Leave confirmed samples alone.
			Self::Confirmed(_) => {},
		}
	}

	/// # Update New Bad Sample.
	///
	/// TBD and Bad samples are simply replaced.
	///
	/// Maybe and Contentious are decremented/downgraded if the value matches
	/// and there is no sync weirdness.
	///
	/// Confirmed stay the same.
	fn update_bad(&mut self, new: Sample, err_sync: bool) {
		match self {
			// Always update a TBD.
			Self::Tbd | Self::Bad(_) => {
				*self = Self::Bad(new);
			},

			// Simple Maybes.
			Self::Maybe((old, count)) =>
				if ! err_sync && new.eq(old) {
					if *count == 1 { *self = Self::Bad(new); }
					else { *count -= 1; }
				},

			// Annoying Vector Maybes.
			Self::Contentious(set) =>
				if ! err_sync {
					let mut changed = false;
					set.retain_mut(|(old, count, _)|
						if new.eq(old) {
							changed = true;
							if *count == 1 { false }
							else {
								*count -= 1;
								true
							}
						}
						else { true }
					);

					if changed {
						let len = set.len();
						// Bad.
						if len == 0 { *self = Self::Bad(new); }
						// Simplify.
						else if len == 1 {
							if set[0].2 {
								*self = Self::Maybe((set[0].0, set[0].1));
							}
						}
						// Re-Sort.
						else {
							set.sort_unstable_by(|a, b| b.1.cmp(&a.1));
						}
					}
				},

			// Leave confirmed samples alone.
			Self::Confirmed(_) => {},
		}
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



#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn t_update() {
		// Start with TBD.
		let mut sample = RipSample::Tbd;
		sample.update(NULL_SAMPLE, true, false);
		assert!(matches!(sample, RipSample::Bad(NULL_SAMPLE)));

		// Bad + Good = Maybe.
		sample.update(NULL_SAMPLE, false, false);
		assert!(matches!(sample, RipSample::Maybe((NULL_SAMPLE, 1))));

		// Maybe + Bad = no change.
		sample.update([1, 1, 1, 1], true, false);
		assert!(matches!(sample, RipSample::Maybe((NULL_SAMPLE, 1))));

		// Maybe + Good = ++
		sample.update(NULL_SAMPLE, false, false);
		assert!(matches!(sample, RipSample::Maybe((NULL_SAMPLE, 2))));

		// Maybe + Good (different) = Contentious
		sample.update([1, 1, 1, 1], false, false);
		{
			let RipSample::Contentious(ref set) = sample else { panic!("Sample should be maybe!"); };
			assert_eq!(set, &[(NULL_SAMPLE, 2, true), ([1, 1, 1, 1], 1, true)]);
		}

		// Contentious + Bad (different) = no change
		sample.update([1, 2, 1, 2], true, false);
		{
			let RipSample::Contentious(ref set) = sample else { panic!("Sample should be maybe!"); };
			assert_eq!(set, &[(NULL_SAMPLE, 2, true), ([1, 1, 1, 1], 1, true)]);
		}

		// Contentious + Bad (existing) = --
		sample.update(NULL_SAMPLE, true, false);
		{
			let RipSample::Contentious(ref set) = sample else { panic!("Sample should be maybe!"); };
			assert_eq!(set, &[(NULL_SAMPLE, 1, true), ([1, 1, 1, 1], 1, true)]);
		}

		// Contentious + Bad (existing) = -- = Maybe
		sample.update(NULL_SAMPLE, true, false);
		assert!(matches!(sample, RipSample::Maybe(([1, 1, 1, 1], 1))));

		// Maybe + Bad (existing) = -- = empty = Bad.
		sample.update([1, 1, 1, 1], true, false);
		assert!(matches!(sample, RipSample::Bad([1, 1, 1, 1])));
	}
}
