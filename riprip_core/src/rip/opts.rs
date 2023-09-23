/*!
# Rip Rip Hooray: Ripping Options
*/

use crate::{
	CD_DATA_SIZE,
	ReadOffset,
};
use std::{
	num::NonZeroU16,
	ops::RangeInclusive,
};
use super::track_idx_to_bits;



/// # FLAG: Read Backwards.
const FLAG_BACKWARDS: u8 =  0b0000_0001;

/// # FLAG: Flip Flop.
const FLAG_FLIP_FLOP: u8 =  0b0000_0010;

/// # FLAG: Reset counts.
const FLAG_RESET: u8 =      0b0000_0100;

/// # FLAG: Resume previous rip (when applicable).
const FLAG_RESUME: u8 =     0b0000_1000;

/// # FLAG: Strict C2 Mode.
const FLAG_STRICT: u8 =     0b0001_0000;

/// # FLAG: Subchannel Sync.
const FLAG_SYNC: u8 =       0b0010_0000;

/// # FLAG: Verbose.
const FLAG_VERBOSE: u8 =    0b0100_0000;

/// # FLAG: Default.
const FLAG_DEFAULT: u8 = FLAG_RESUME;

/// # Minimum Confidence.
const CONFIDENCE_MIN: u8 = 3;

/// # Maximum Confidence.
const CONFIDENCE_MAX: u8 = 10;

/// # Maximum Refine Passes.
const PASSES_MAX: u8 = 16;

/// # Reread Absolute Max.
const REREADS_ABS_MAX: u8 = 20;

/// # Reread Multiplier Max.
const REREADS_REL_MAX: u8 = 10;



#[derive(Debug, Clone, Copy)]
/// # Rip Options.
///
/// This struct holds the rip-related options like read offset, track numbers,
/// etc.
///
/// Options are set using builder-style methods, like:
///
/// ```
/// use riprip_core::RipOptions;
///
/// let opts = RipOptions::default()
///     .with_passes(3)
///     .with_track(3) // Order doesn't matter.
///     .with_track(2)
///     .with_track(2) // Duplication doesn't matter.
///     .with_track(15);
///
/// assert_eq!(opts.passes(), 3);
/// assert_eq!(opts.tracks().collect::<Vec<u8>>(), &[2, 3, 15]);
/// ```
pub struct RipOptions {
	offset: ReadOffset,
	cache: Option<NonZeroU16>,
	confidence: u8,
	rereads: (u8, u8),
	passes: u8,
	flags: u8,
	tracks: u128,
}

impl Default for RipOptions {
	fn default() -> Self {
		Self {
			offset: ReadOffset::default(),
			cache: None,
			confidence: 3,
			rereads: (2, 2),
			passes: 1,
			flags: FLAG_DEFAULT,
			tracks: 0,
		}
	}
}

macro_rules! with_flag {
	($fn:ident, $flag:ident, $($doc:literal),+ $(,)?) => (
		#[must_use]
		$(
			#[doc = $doc]
		)+
		pub const fn $fn(self, v: bool) -> Self {
			let flags =
				if v { self.flags | $flag }
				else { self.flags & ! $flag };

			Self {
				flags,
				..self
			}
		}
	)
}

/// ## Setters.
impl RipOptions {
	with_flag!(
		with_backwards,
		FLAG_BACKWARDS,
		"# Rip Backwards.",
		"",
		"When `true`, track sectors will be read last to first instead of the",
		"usual way.",
		"",
		"The default is `false`.",
	);

	#[must_use]
	/// # With Cache Size.
	///
	/// Set the drive's read buffer cache size in KiB (1024 bytes) so that it
	/// can be (probably) effectively cleared before reading from a track.
	///
	/// Set to zero to disable. Also the default.
	pub const fn with_cache(self, cache: u16) -> Self {
		Self {
			cache: NonZeroU16::new(cache),
			..self
		}
	}

	#[must_use]
	/// # Confirmation Confidence.
	///
	/// To avoid unnecessary repetition, track rip checksums are checked
	/// against the third-party AccurateRip and CUETools databases for
	/// independent verification.
	///
	/// If the match confidence from either of those is at least this value,
	/// the data will be considered confirmed and no further ripping attempts
	/// will be made.
	///
	/// Values are capped to `3..=10`, with a default of `3`.
	///
	/// The default should be sufficient in nearly all cases, but if you
	/// personally polluted the databases with prior bad rips, you can nudge it
	/// a little higher to avoid matching your past self. ;)
	pub const fn with_confidence(self, mut confidence: u8) -> Self {
		if confidence < CONFIDENCE_MIN { confidence = CONFIDENCE_MIN; }
		else if CONFIDENCE_MAX < confidence { confidence = CONFIDENCE_MAX; }
		Self {
			confidence,
			..self
		}
	}

	with_flag!(
		with_flip_flop,
		FLAG_FLIP_FLOP,
		"# Alternate Rip Read Order.",
		"",
		"When `true`, the sector read order will alternate between passes,",
		"flipping from forward to backward to forward to backward…",
		"",
		"The default is `false`.",
	);

	#[must_use]
	/// # Read Offset.
	///
	/// Optical drives have weirdly arbitrary precision problems, causing them
	/// to read data a little earlier or later than another drive might.
	///
	/// To normalize the data obtained across different drives, it is critical
	/// to set the appropriate count-offset. See [here](http://www.accuraterip.com/driveoffsets.htm) if you're not sure
	/// what your drive's offset is.
	pub const fn with_offset(self, offset: ReadOffset) -> Self {
		Self {
			offset,
			..self
		}
	}

	#[must_use]
	/// # Number of Passes.
	///
	/// Rip Rip rips are indefinitely iterable, but that iteration can also be
	/// automated by using this method to set more than the usual number of
	/// passes.
	///
	/// The behaviors are the same whether secondary passes occur due to this
	/// setting or manually re-running the program.
	///
	/// Passes don't re-rip data unnecessarily, so this setting will have no
	/// effect on tracks that contain nothing but likely/confirmed samples.
	///
	/// The default is `1`.
	///
	/// Values are capped to `1..=16`, but the program can be manually re-run
	/// if even more passes are needed. ;)
	pub const fn with_passes(self, mut passes: u8) -> Self {
		if 0 == passes { passes = 1; }
		else if PASSES_MAX < passes { passes = PASSES_MAX; }
		Self {
			passes,
			..self
		}
	}

	#[must_use]
	/// # Likeliness Re-Read Cutoff.
	///
	/// Drives may return different values for a given sample from read-to-read
	/// due to… issues, but at a certain point it becomes necessary to call good
	/// enough "Good Enough".
	///
	/// There are two components to re-read qualification: an absolute total
	/// relative multiplier.
	///
	/// The absolute total is simply the number of times a given value has been
	/// returned for a given sample. An `abs` of two means we need to see the
	/// same value at least twice.
	///
	/// The relative multiplier helps lessen contention with competing values.
	/// The leading value must have appeared more than `rel` times as often as
	/// any contradictory values. A `rel` of two means the total of the leading
	/// value must be more than twice that of the combined total of any other
	/// random crap returned by the drive.
	///
	/// When all samples in a sector meet this criteria, that sector is skipped
	/// during re-ripping.
	///
	/// The default is `2`/`2`.
	///
	/// Values are capped to `1..=20` and `1..=10` respectively.
	pub const fn with_rereads(self, mut abs: u8, mut rel: u8) -> Self {
		if abs == 0 { abs = 1; }
		else if REREADS_ABS_MAX < abs { abs = REREADS_ABS_MAX; }

		if rel == 0 { rel = 1; }
		else if REREADS_REL_MAX < rel { rel = REREADS_REL_MAX; }

		Self {
			rereads: (abs, rel),
			..self
		}
	}

	with_flag!(
		with_reset,
		FLAG_RESET,
		"# Reset Counts.",
		"",
		"When `true`, reset all previously-collected sample counts to 1,",
		"downgrading all likely values to maybe.",
		"",
		"The default is `false`.",
	);

	with_flag!(
		with_resume,
		FLAG_RESUME,
		"# Resume Previous Rip.",
		"",
		"When `true`, if state data exists for the track, Rip Rip Hooray!",
		"will pick up from where it left off. When `false`, it will start",
		"over from scratch.",
		"",
		"The default is `true`.",
	);

	with_flag!(
		with_strict,
		FLAG_STRICT,
		"# Strict C2 (Sector).",
		"",
		"When `true`, C2 errors are an all-or-nothing proposition for the sector",
		"as a whole. If any sample is bad, all samples are bad.",
		"",
		"The default is `false`.",
	);

	with_flag!(
		with_sync,
		FLAG_SYNC,
		"# Require Subchannel Sync.",
		"",
		"When `true`, sector data will only be accepted if the subchannel",
		"timecode matches the requested LSN; if there's a desync, data will be",
		"ignored and tried again on subsequent passes.",
		"",
		"Subchannel data is easily corrupted, so this is only potentially",
		"useful in cases where disc rot, rather than wear-and-tear, is the sole",
		"cause of readability issues.",
		"",
		"The default is `false`.",
	);

	#[must_use]
	/// # Include Track.
	///
	/// Add a given track number to the to-rip list.
	pub const fn with_track(self, track: u8) -> Self {
		let tracks = self.tracks | track_idx_to_bits(track);
		Self {
			tracks,
			..self
		}
	}

	#[must_use]
	/// # Exclude Track.
	///
	/// Remove a given track number from the to-rip list.
	pub const fn without_track(self, track: u8) -> Self {
		let flag = track_idx_to_bits(track);
		if flag == 0 { self }
		else {
			let tracks = self.tracks & ! flag;
			Self {
				tracks,
				..self
			}
		}
	}

	with_flag!(
		with_verbose,
		FLAG_VERBOSE,
		"# Verbose (Log) Mode.",
		"",
		"When `true`, detailed sector quality issues will be printed to STDOUT",
		"so they can be piped to a file for review.",
		"",
		"The default is `false`.",
	);
}



macro_rules! get_flag {
	($fn:ident, $flag:ident, $title:literal) => (
		#[must_use]
		#[doc = concat!("# ", $title, "?")]
		pub const fn $fn(&self) -> bool { $flag == self.flags & $flag }
	);
}

/// # Getters.
impl RipOptions {
	get_flag!(backwards, FLAG_BACKWARDS, "Rip Backwards");
	get_flag!(strict, FLAG_STRICT, "Strict C2 Error Pointers");
	get_flag!(flip_flop, FLAG_FLIP_FLOP, "Alternate Rip Read Order");
	get_flag!(reset, FLAG_RESET, "Reset Counts");
	get_flag!(resume, FLAG_RESUME, "Resume Previous Rip");
	get_flag!(sync, FLAG_SYNC, "Subchannel Sync");
	get_flag!(verbose, FLAG_VERBOSE, "Verbose (Log) Mode");

	#[must_use]
	/// # Cache Size.
	pub const fn cache(&self) -> Option<NonZeroU16> { self.cache }

	#[must_use]
	#[allow(clippy::integer_division)]
	/// # Cache Sectors.
	///
	/// Return the cache size in sectors, rounded up.
	///
	/// TODO: use `div_ceil` once it becomes available.
	pub const fn cache_sectors(&self) -> u32 {
		if let Some(c) = self.cache {
			c.get() as u32 * 1024 / CD_DATA_SIZE as u32 + 1
		}
		else { 0 }
	}

	#[must_use]
	/// # Minimum AccurateRip/CTDB Confidence.
	pub const fn confidence(&self) -> u8 { self.confidence }

	#[must_use]
	/// # Has Any Tracks?
	pub const fn has_tracks(&self) -> bool { self.tracks != 0 }

	#[must_use]
	/// # Read Offset.
	pub const fn offset(&self) -> ReadOffset { self.offset }

	#[must_use]
	/// # Number of Passes.
	pub const fn passes(&self) -> u8 { self.passes }

	#[must_use]
	/// # Likeliness Reread Cutoffs.
	pub const fn rereads(&self) -> (u8, u8) { self.rereads }

	#[must_use]
	/// # Tracks.
	///
	/// Return an iterator over the included track indices.
	pub const fn tracks(&self) -> RipOptionsTracks {
		RipOptionsTracks {
			set: self.tracks,
			pos: 0,
		}
	}

	#[must_use]
	/// # Tracks.
	///
	/// Return an iterator over the included track indices, collapsed into
	/// inclusive ranges.
	pub const fn tracks_rng(&self) -> RipOptionsTracksRng {
		RipOptionsTracksRng {
			set: self.tracks,
			pos: 0,
		}
	}
}

#[cfg(feature = "bin")]
/// # Misc.
impl RipOptions {
	#[must_use]
	/// # CLI String.
	///
	/// Convert the options back into a list of arguments in CLI format. This
	/// code isn't super pretty, but it's pretty straightforward.
	pub fn cli(&self) -> String {
		use std::borrow::Cow;

		// The entries will be variable, so toss them into a vec first.
		let mut opts = Vec::new();

		// All the easy stuff.
		if self.backwards() { opts.push(Cow::Borrowed("--backwards")); }
		if let Some(cache) = self.cache {
			opts.push(Cow::Owned(format!("-c{cache}")));
		}
		opts.push(Cow::Owned(format!("--confidence={}", self.confidence())));
		if self.flip_flop() { opts.push(Cow::Borrowed("--flip-flop")); }
		if ! self.resume() { opts.push(Cow::Borrowed("--no-resume")); }
		let offset = self.offset().samples();
		if offset != 0 { opts.push(Cow::Owned(format!("-o{offset}"))); }
		opts.push(Cow::Owned(format!("-p{}", self.passes())));
		let rr = self.rereads();
		opts.push(Cow::Owned(format!("-r{},{}", rr.0, rr.1)));
		if self.reset() { opts.push(Cow::Borrowed("--reset-counts")); }
		if self.strict() { opts.push(Cow::Borrowed("--strict-c2")); }
		if self.sync() { opts.push(Cow::Borrowed("--sync")); }

		// The tracks should be condensed.
		opts.push(Cow::Owned(format!(
			"-t{}",
			self.tracks_rng().map(|rng| {
				let (a, b) = rng.into_inner();
				if a == b { a.to_string() }
				else { format!("{a}-{b}") }
			}).collect::<Vec<String>>().join(",")
		)));

		// Done!
		opts.join(" ")
	}
}


#[derive(Debug, Clone)]
/// # Rip Option Tracks.
///
/// This iterator converts the `u128` monster flag back into individual `u8`
/// track indexes.
pub struct RipOptionsTracks {
	set: u128,
	pos: u8,
}

impl Iterator for RipOptionsTracks {
	type Item = u8;

	fn next(&mut self) -> Option<Self::Item> {
		while self.pos < 100 {
			let idx = self.pos;
			self.pos += 1;
			if 0 != self.set & track_idx_to_bits(idx) {
				return Some(idx);
			}
		}
		None
	}

	/// # Size Hint.
	///
	/// There will never be more than 99 tracks.
	fn size_hint(&self) -> (usize, Option<usize>) {
		(0, Some(100_usize.saturating_sub(usize::from(self.pos))))
	}
}



#[derive(Debug, Clone)]
/// # Rip Option Tracks (As Range).
///
/// Like [`RipOptionsTracks`], but results are returned as ranges instead of
/// individual numbers. Useful for compact display, I suppose.
pub struct RipOptionsTracksRng {
	set: u128,
	pos: u8,
}

impl Iterator for RipOptionsTracksRng {
	type Item = RangeInclusive<u8>;

	fn next(&mut self) -> Option<Self::Item> {
		let mut from = u8::MAX;
		let mut to = u8::MAX;

		while self.pos < 100 {
			let idx = self.pos;
			if 0 != self.set & track_idx_to_bits(idx) {
				if from == u8::MAX {
					from = idx;
					to = idx;
				}
				else if to + 1 == idx {
					to = idx;
				}
				else {
					return Some(from..=to);
				}
			}
			self.pos += 1;
		}

		if from == u8::MAX { None }
		else { Some(from..=to) }
	}

	/// # Size Hint.
	///
	/// There will never be more than 99 tracks.
	fn size_hint(&self) -> (usize, Option<usize>) {
		(0, Some(100_usize.saturating_sub(usize::from(self.pos))))
	}
}



#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn t_rip_flags() {
		// Make sure our flags are unique.
		let mut all = vec![
			FLAG_BACKWARDS,
			FLAG_FLIP_FLOP,
			FLAG_RESET,
			FLAG_RESUME,
			FLAG_STRICT,
			FLAG_SYNC,
			FLAG_VERBOSE,
		];
		all.sort_unstable();
		all.dedup();
		assert_eq!(all.len(), 7);

		// Also make sure each is only one bit.
		assert!(all.iter().all(|&v| v.count_ones() == 1));
	}

	#[test]
	fn t_rip_options_cache() {
		let mut opts = RipOptions::default();
		assert_eq!(opts.cache(), None);
		opts = opts.with_cache(16);
		assert_eq!(opts.cache(), NonZeroU16::new(16));
		opts = opts.with_cache(0);
		assert_eq!(opts.cache(), None);
	}

	#[test]
	fn t_rip_options_confidence() {
		for v in [3, 4, 5] {
			let opts = RipOptions::default().with_confidence(v);
			assert_eq!(opts.confidence(), v);
		}

		// Min.
		let opts = RipOptions::default().with_confidence(0);
		assert_eq!(opts.confidence(), CONFIDENCE_MIN);

		// Max.
		let opts = RipOptions::default().with_confidence(64);
		assert_eq!(opts.confidence(), CONFIDENCE_MAX);
	}

	#[test]
	fn t_rip_options_flags() {
		macro_rules! t_flags {
			($name:literal, $set:ident, $get:ident) => (
				let mut opts = RipOptions::default();
				for v in [false, true, false, true] {
					opts = opts.$set(v);
					assert_eq!(
						opts.$get(),
						v,
						concat!("Setting ", $name, " to {} failed."),
						v
					);
				}
			);
		}

		t_flags!("backwards", with_backwards, backwards);
		t_flags!("flip_flop", with_flip_flop, flip_flop);
		t_flags!("reset", with_reset, reset);
		t_flags!("resume", with_resume, resume);
		t_flags!("strict", with_strict, strict);
		t_flags!("sync", with_sync, sync);
		t_flags!("verbose", with_verbose, verbose);
	}

	#[test]
	fn t_rip_options_offset() {
		let offset5 = ReadOffset::try_from(b"5".as_slice()).expect("Read offset 5 failed.");
		let offset667 = ReadOffset::try_from(b"-667".as_slice()).expect("Read offset -667 failed.");
		for v in [offset5, offset667] {
			let opts = RipOptions::default().with_offset(v);
			assert_eq!(opts.offset(), v);
		}
	}

	#[test]
	fn t_rip_options_passes() {
		for v in [1, 2, 3] {
			let opts = RipOptions::default().with_passes(v);
			assert_eq!(opts.passes(), v);
		}

		// Min.
		let opts = RipOptions::default().with_passes(0);
		assert_eq!(opts.passes(), 1);

		// Max.
		let opts = RipOptions::default().with_passes(64);
		assert_eq!(opts.passes(), PASSES_MAX);
	}

	#[test]
	fn t_rip_options_rereads() {
		for (a, b) in [(1, 2), (2, 3), (3, 4)] {
			let opts = RipOptions::default().with_rereads(a, b);
			assert_eq!(opts.rereads(), (a, b));
		}

		// Min/Max.
		let opts = RipOptions::default().with_rereads(0, 0);
		assert_eq!(opts.rereads(), (1, 1));
		let opts = RipOptions::default().with_rereads(64, 64);
		assert_eq!(opts.rereads(), (REREADS_ABS_MAX, REREADS_REL_MAX));
	}

	#[test]
	fn t_rip_options_tracks() {
		let mut opts = RipOptions::default();
		assert!(! opts.has_tracks(), "The track list should be empty!");

		// Make sure zero counts.
		opts = opts.with_track(0);
		assert!(opts.has_tracks(), "Zero should count!");

		// Make sure 100 isn't allowed.
		assert_eq!(track_idx_to_bits(100), 0, "100 shouldn't have a track flag.");

		// Add all possible tracks.
		for idx in 0..=u8::MAX { opts = opts.with_track(idx); }
		assert!(opts.has_tracks(), "The track list should not be empty!");

		// Pull them back.
		let tracks = opts.tracks().collect::<Vec<u8>>();
		assert_eq!(tracks.len(), 100, "Expected 100 tracks.");

		// Make sure everything is where we expect it to be.
		for (real, expected) in tracks.into_iter().zip(0..=99_u8) {
			assert_eq!(real, expected, "Options track mismatch: {real} instead of {expected}.");
		}

		// Make sure this works with a somewhat random list.
		let expected = [0, 5, 15];
		opts = RipOptions::default();
		assert!(! opts.has_tracks(), "The track list should be empty!");
		for idx in expected { opts = opts.with_track(idx); }
		let real = opts.tracks().collect::<Vec<u8>>();
		assert_eq!(real.len(), expected.len(), "Expected {} tracks.", expected.len());
		for (real, expected) in real.into_iter().zip(expected) {
			assert_eq!(real, expected, "Options track mismatch: {real} instead of {expected}.");
		}

		// Remove 0.
		opts = opts.without_track(0);
		let expected = [5, 15];
		let real = opts.tracks().collect::<Vec<u8>>();
		assert_eq!(real.len(), expected.len(), "Expected {} tracks.", expected.len());
		for (real, expected) in real.into_iter().zip(expected) {
			assert_eq!(real, expected, "Options track mismatch: {real} instead of {expected}.");
		}

		// Remove the rest.
		opts = opts.without_track(5).without_track(15);
		assert!(! opts.has_tracks(), "Options tracks should be empty!");
	}

	#[test]
	fn t_track_rng() {
		let mut opts = RipOptions::default();
		for i in [1, 2, 3, 6, 10, 11] { opts = opts.with_track(i); }

		let mut rng = opts.tracks_rng();
		assert_eq!(rng.next(), Some(1..=3));
		assert_eq!(rng.next(), Some(6..=6));
		assert_eq!(rng.next(), Some(10..=11));
		assert_eq!(rng.next(), None);

		opts = RipOptions::default();
		assert_eq!(opts.tracks_rng().next(), None);

		opts = opts.with_track(4);
		rng = opts.tracks_rng();
		assert_eq!(rng.next(), Some(4..=4));
		assert_eq!(rng.next(), None);
	}
}
