/*!
# Rip Rip Hooray: Ripping Options
*/

use crate::{
	CD_C2_SIZE,
	CD_C2B_SIZE,
	ReadOffset,
};



/// # FLAG: Rip Backwards.
const FLAG_BACKWARDS: u8 =    0b0000_0001;

/// # FLAG: Cache Bust.
const FLAG_CACHE_BUST: u8 =   0b0000_0010;

/// # FLAG: RAW PCM (instead of WAV).
const FLAG_RAW: u8 =          0b0000_0100;

/// # FLAG: Reset counts.
const FLAG_RESET_COUNTS: u8 = 0b0000_1000;

/// # FLAG: Resume previous rip (when applicable).
const FLAG_RESUME: u8 =       0b0001_0000;

/// # FLAG: Strict Mode.
const FLAG_STRICT: u8 =       0b0010_0000;

/// # FLAG: Default.
const FLAG_DEFAULT: u8 = FLAG_CACHE_BUST | FLAG_RESUME;

/// # Minimum Confidence.
const CONFIDENCE_MIN: u8 = 3;

/// # Maximum Confidence.
const CONFIDENCE_MAX: u8 = 10;

/// # Maximum Likely Level.
const CUTOFF_MAX: u8 = 32;

/// # Maximum Refine Passes.
const REFINE_MAX: u8 = 32;



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
///     .with_refine(3)
///     .with_track(3) // Order doesn't matter.
///     .with_track(2)
///     .with_track(15);
///
/// assert_eq!(opts.refine(), 3);
/// assert_eq!(opts.tracks().collect::<Vec<u8>>(), &[2, 3, 15]);
/// ```
pub struct RipOptions {
	offset: ReadOffset,
	c2: RipOptionsC2,
	confidence: u8,
	cutoff: u8,
	refine: u8,
	flags: u8,
	tracks: u128,
}

impl Default for RipOptions {
	fn default() -> Self {
		Self {
			offset: ReadOffset::default(),
			c2: RipOptionsC2::default(),
			confidence: 3,
			cutoff: 2,
			refine: 0,
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
		"When `true`, read sectors in reverse order.",
		"",
		"The default is `false`.",
	);

	#[must_use]
	/// # C2 Error Pointers.
	///
	/// Set the C2 mode or disable it altogether, although if the latter, be
	/// warned that data accuracy will be really hard to verify.
	///
	/// By default, 294-byte C2 error support is assumed.
	pub const fn with_c2(self, c2: RipOptionsC2) -> Self {
		Self {
			c2,
			..self
		}
	}

	with_flag!(
		with_cache_bust,
		FLAG_CACHE_BUST,
		"# Bust Cache Between Passes.",
		"",
		"To ensure the drive actually _reads_ the data being requested,",
		"Rip Rip Hooray! will attempt to flush the cache at the start of",
		"each rip pass (*not* after every single read, as other rippers do).",
		"",
		"The default is `true`.",
	);

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

	#[must_use]
	/// # Likeliness Cutoff.
	///
	/// Drives may return different values for a given sample from read-to-read
	/// due to… issues, but at a certain point it becomes necessary to call good
	/// enough "Good Enough".
	///
	/// When a given value is returned this many times, and twice as often as
	/// all competing values, Rip Rip Hooray! won't try to re-read it any more.
	///
	/// The lower the setting, the less work there will be to do on subsequent
	/// passes; the higher the setting, the better the rip will be able to cope
	/// with drive wishywashiness.
	///
	/// This also has no _practical_ effect unless all samples in a given
	/// sector are likely/confirmed; if any require re-reading, the sector as a
	/// whole will still need to be reread.
	///
	/// The value can be adjusted from run-to-run as needed.
	///
	/// The default is `2`, which is a reasonable starting point. Values are
	/// capped automatically to `1..=32`.
	pub const fn with_cutoff(self, mut cutoff: u8) -> Self {
		if cutoff == 0 { cutoff = 1; }
		else if CUTOFF_MAX < cutoff { cutoff = CUTOFF_MAX; }
		Self {
			cutoff,
			..self
		}
	}

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

	with_flag!(
		with_raw,
		FLAG_RAW,
		"# Output Raw PCM.",
		"",
		"When `true`, tracks will be saved in raw PCM format. When `false`,",
		"they'll be saved as WAV files.",
		"",
		"The default is `false`.",
	);

	#[must_use]
	/// # Automated Refinement.
	///
	/// Rip Rip rips are indefinitely resumable, but if know it'll take
	/// multiple passes to capture the data, you can use this option to
	/// automate re-ripping up to this many times for each track.
	///
	/// The process will stop early if the track rip is verifiable — matches
	/// AccurateRip and/or CUETools — or all samples meet the likeliness
	/// threshold.
	///
	/// The default is `0`.
	///
	/// To give the drive a break, the maximum value is capped at `32`, but you
	/// can manually rerun the program afterward as many times as needed. ;)
	pub const fn with_refine(self, mut refine: u8) -> Self {
		if REFINE_MAX < refine { refine = REFINE_MAX; }
		Self {
			refine,
			..self
		}
	}

	with_flag!(
		with_reset_counts,
		FLAG_RESET_COUNTS,
		"# Reset Counts.",
		"",
		"When `true`, all previously-collected sample counts, downgrading all",
		"likely values to maybe.",
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
		"# Strict Mode.",
		"",
		"When `true`, if a sector contains _any_ C2 errors, all samples in the",
		"response are considered bad. When `false`, sample goodness is judged",
		"individually.",
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
	get_flag!(cache_bust, FLAG_CACHE_BUST, "Bust Cache");
	get_flag!(raw, FLAG_RAW, "Output Raw PCM");
	get_flag!(reset_counts, FLAG_RESET_COUNTS, "Reset Counts");
	get_flag!(resume, FLAG_RESUME, "Resume Previous Rip");
	get_flag!(strict, FLAG_STRICT, "Strict Mode");

	#[must_use]
	/// # C2 Error Pointer Mode.
	pub const fn c2(&self) -> RipOptionsC2 { self.c2 }

	#[must_use]
	/// # Minimum AccurateRip/CTDB Confidence.
	pub const fn confidence(&self) -> u8 { self.confidence }

	#[must_use]
	/// # Likeliness Cutoff.
	pub const fn cutoff(&self) -> u8 { self.cutoff }

	#[must_use]
	/// # Has Any Tracks?
	pub const fn has_tracks(&self) -> bool { self.tracks != 0 }

	#[must_use]
	/// # Read Offset.
	pub const fn offset(&self) -> ReadOffset { self.offset }

	#[must_use]
	/// # Number of Passes.
	///
	/// This is always `1` + [`RipOptions::refine`].
	pub const fn passes(&self) -> u8 { self.refine + 1 }

	#[must_use]
	/// # Number of Refine Passes.
	pub const fn refine(&self) -> u8 { self.refine }

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
}



#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
#[repr(u16)]
/// # Rip Option C2.
///
/// Lest data accuracy be _too_ easy, there are two different ways of handling
/// C2 error pointers, plus the possibility they won't be handled at all. Haha.
pub enum RipOptionsC2 {
	/// # No C2 Support.
	None = 0,

	#[default]
	/// # 294-byte Block.
	C2Mode294 = CD_C2_SIZE,

	/// # 296-byte Block.
	C2Mode296 = CD_C2B_SIZE,
}

impl RipOptionsC2 {
	#[must_use]
	/// # Is None?
	pub const fn is_none(self) -> bool { matches!(self, Self::None) }
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



/// # Track Number to Bitflag.
///
/// Redbook audio CDs can only have a maximum of 99 tracks — or 100 if we count
/// the HTOA as #0 — so we can represent all possible combinations using a
/// single `u128` bitflag. Aside from being `Copy`, this saves us the trouble
/// of having to sort/dedup some sort of vector-like structure.
///
/// This method converts a `u8` decimal into the equivalent flag. Out of range
/// values are silently treated as zero.
const fn track_idx_to_bits(idx: u8) -> u128 {
	if 99 < idx { 0 }
	else { 2_u128.pow(idx as u32) }
}



#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn t_rip_flags() {
		// Make sure our flags are unique.
		let mut all = vec![
			FLAG_BACKWARDS,
			FLAG_CACHE_BUST,
			FLAG_RAW,
			FLAG_RESET_COUNTS,
			FLAG_RESUME,
			FLAG_STRICT,
		];
		all.sort_unstable();
		all.dedup();
		assert_eq!(all.len(), 6);
	}

	#[test]
	fn t_c2() {
		let mut opts = RipOptions::default();
		for v in [RipOptionsC2::None, RipOptionsC2::C2Mode294, RipOptionsC2::C2Mode296] {
			opts = opts.with_c2(v);
			assert_eq!(opts.c2(), v);
		}
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
	fn t_rip_options_cutoff() {
		for v in [1, 2, 3] {
			let opts = RipOptions::default().with_cutoff(v);
			assert_eq!(opts.cutoff(), v);
		}

		// Min.
		let opts = RipOptions::default().with_cutoff(0);
		assert_eq!(opts.cutoff(), 1);

		// Max.
		let opts = RipOptions::default().with_cutoff(64);
		assert_eq!(opts.cutoff(), CUTOFF_MAX);
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
		t_flags!("cache bust", with_cache_bust, cache_bust);
		t_flags!("raw", with_raw, raw);
		t_flags!("reset_counts", with_reset_counts, reset_counts);
		t_flags!("resume", with_resume, resume);
		t_flags!("strict", with_strict, strict);
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
	fn t_rip_options_refine() {
		for v in [0, 1, 2, 3] {
			let opts = RipOptions::default().with_refine(v);
			assert_eq!(opts.refine(), v);
			assert_eq!(opts.passes(), v + 1);
		}

		// Max.
		let opts = RipOptions::default().with_refine(64);
		assert_eq!(opts.refine(), REFINE_MAX);
		assert_eq!(opts.passes(), REFINE_MAX + 1);
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
	}
}
