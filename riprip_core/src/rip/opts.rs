/*!
# Rip Rip Hooray: Ripping Options
*/

use crate::ReadOffset;



/// # FLAG: C2 Support.
const FLAG_C2: u8 =         0b000_0001;

/// # FLAG: Cache Bust.
const FLAG_CACHE_BUST: u8 = 0b000_0010;

/// # FLAG: RAW PCM (instead of WAV).
const FLAG_RAW: u8 =        0b000_0100;

/// # FLAG: Reconfirm samples.
const FLAG_RECONFIRM: u8 =  0b000_1000;

/// # FLAG: Trust Good Sectors.
const FLAG_TRUST: u8 =      0b001_0000;

/// # FLAG: Default.
const FLAG_DEFAULT: u8 = FLAG_C2 | FLAG_CACHE_BUST | FLAG_TRUST;



#[derive(Debug, Clone, Copy)]
/// # Rip Options.
///
/// This struct holds the rip-related options like read offset, paranoia level,
/// which tracks to focus on, etc.
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
	paranoia: u8,
	refine: u8,
	flags: u8,
	tracks: u128,
}

impl Default for RipOptions {
	fn default() -> Self {
		Self {
			offset: ReadOffset::default(),
			paranoia: 3,
			refine: 0,
			flags: FLAG_DEFAULT,
			tracks: 0,
		}
	}
}

impl RipOptions {
	#[must_use]
	/// # With Offset.
	///
	/// Set the AccurateRip, _et al_, drive read offset to apply when copying
	/// data from the disc. See [here](http://www.accuraterip.com/driveoffsets.htm) for more information.
	///
	/// It is critical the correct offset be applied, otherwise the contents of
	/// the rip may not be independently verifiable. This is doubly so when two
	/// or more drives are used for a single rip; without appropriate offsets
	/// the communal data could be corrupted.
	///
	/// The default is zero.
	pub const fn with_offset(self, offset: ReadOffset) -> Self {
		Self {
			offset,
			..self
		}
	}

	#[must_use]
	/// # With C2 Error Pointers.
	///
	/// Enable or disable the use of C2 error pointer information.
	///
	/// This feature is critical for ensuring any degree of transfer accuracy,
	/// but if a drive doesn't support it, it should be disabled.
	///
	/// The default is enabled.
	pub const fn with_c2(self, c2: bool) -> Self {
		let flags =
			if c2 { self.flags | FLAG_C2 }
			else { self.flags & ! FLAG_C2 };

		Self {
			flags,
			..self
		}
	}

	#[must_use]
	/// # With Cache Bust.
	///
	/// Enable or disable cache busting. (Rip Rip will try to circumvent the
	/// drive cache by having it first read random data from somewhere else.)
	///
	/// Unlike with other CD-rippers, Rip Rip only needs to cache bust once per
	/// track per pass, not after every single read. Its impact on performance
	/// and on the drive should almost always be negligible.
	///
	/// The default is enabled.
	pub const fn with_cache_bust(self, cache_bust: bool) -> Self {
		let flags =
			if cache_bust { self.flags | FLAG_CACHE_BUST }
			else { self.flags & ! FLAG_CACHE_BUST };

		Self {
			flags,
			..self
		}
	}

	#[must_use]
	/// # With Paranoia Level.
	///
	/// Whenever a drive reports _any_ C2 or read errors for a block, consider
	/// _all_ samples in that block — namely the allegedly good ones — as
	/// suspicious until the same values have been returned this many times.
	///
	/// The default is three.
	///
	/// Custom values are automatically capped at `1..=32`.
	pub const fn with_paranoia(self, mut paranoia: u8) -> Self {
		if paranoia == 0 { paranoia = 1; }
		else if paranoia > 32 { paranoia = 32; }
		Self {
			paranoia,
			..self
		}
	}

	#[must_use]
	/// # With Raw PCM Output.
	///
	/// When `true`, tracks will be exported in raw PCM format. When `false`,
	/// they'll be saved as WAV files instead.
	///
	/// The default is `false`.
	pub const fn with_raw(self, raw: bool) -> Self {
		let flags =
			if raw { self.flags | FLAG_RAW }
			else { self.flags & ! FLAG_RAW };

		Self {
			flags,
			..self
		}
	}

	#[must_use]
	/// # With Reconfirmation.
	///
	/// If true, previously-accepted samples will be "downgraded" to
	/// "suspicious", requring reconfirmation from subsequent reads.
	///
	/// The default is disabled.
	pub const fn with_reconfirm(self, reconfirm: bool) -> Self {
		let flags =
			if reconfirm { self.flags | FLAG_RECONFIRM }
			else { self.flags & ! FLAG_RECONFIRM };

		Self {
			flags,
			..self
		}
	}

	#[must_use]
	/// # With Refine Passes.
	///
	/// Execute this many additional rip passes so long as any samples remain
	/// unread or unconfirmed. This is equivalent to re-running the entire
	/// program X number of times, but saves you the trouble of having to do
	/// that.
	///
	/// The default is zero; the max is `15`, just to give the drive a break.
	pub const fn with_refine(self, mut refine: u8) -> Self {
		if refine > 15 { refine = 15; }
		Self {
			refine,
			..self
		}
	}

	#[must_use]
	/// # With Track.
	///
	/// Add a track to the rip list.
	pub const fn with_track(self, track: u8) -> Self {
		let tracks = self.tracks | track_idx_to_bits(track);
		Self {
			tracks,
			..self
		}
	}

	#[must_use]
	/// # With Trust.
	///
	/// When `true`, `paranoia` confirmation is only applied to samples within
	/// a sector containing C2 errors. When `false`, every sample is subject to
	/// confirmation before being accepted.
	///
	/// The default is `true`.
	pub const fn with_trust(self, trust: bool) -> Self {
		let flags =
			if trust { self.flags | FLAG_TRUST }
			else { self.flags & ! FLAG_TRUST };

		Self {
			flags,
			..self
		}
	}
}

impl RipOptions {
	#[must_use]
	/// # Offset.
	pub const fn offset(&self) -> ReadOffset { self.offset }

	#[must_use]
	/// # Use C2 Error Pointers?
	pub const fn c2(&self) -> bool { FLAG_C2 == self.flags & FLAG_C2 }

	#[must_use]
	/// # Bust Cache?
	pub const fn cache_bust(&self) -> bool { FLAG_CACHE_BUST == self.flags & FLAG_CACHE_BUST }

	#[must_use]
	/// # Has Tracks?
	pub const fn has_tracks(&self) -> bool { self.tracks != 0 }

	#[must_use]
	/// # Paranoia Level.
	pub const fn paranoia(&self) -> u8 { self.paranoia }

	#[must_use]
	/// # Number of Passes.
	///
	/// Return the total number of passes, e.g. `1 + refine`.
	pub const fn passes(&self) -> u8 { self.refine() + 1 }

	#[must_use]
	/// # Save as Raw PCM?
	pub const fn raw(&self) -> bool { FLAG_RAW == self.flags & FLAG_RAW }

	#[must_use]
	/// # Require Reconfirmation?
	pub const fn reconfirm(&self) -> bool { FLAG_RECONFIRM == self.flags & FLAG_RECONFIRM }

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

	#[must_use]
	/// # Trust Good Sectors?
	pub const fn trust(&self) -> bool { FLAG_TRUST == self.flags & FLAG_TRUST }
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
			self.pos += 1;
			if 0 != self.set & track_idx_to_bits(self.pos) {
				return Some(self.pos);
			}
		}
		None
	}

	/// # Size Hint.
	///
	/// There will never be more than 99 tracks.
	fn size_hint(&self) -> (usize, Option<usize>) {
		(0, Some(99_usize.saturating_sub(usize::from(self.pos))))
	}
}



#[allow(clippy::too_many_lines)]
/// # Track Number to Bitflag.
///
/// Redbook audio CDs can only have a maximum of 99 tracks, so we can represent
/// all possible combinations using a single `u128` bitflag. Aside from being
/// `Copy`, this saves us the trouble of having to sort/dedup some sort of
/// vector-like structure.
///
/// This method converts a `u8` decimal into the equivalent flag. Out of range
/// values are silently treated as zero.
const fn track_idx_to_bits(idx: u8) -> u128 {
	if 0 == idx || idx > 99 { 0 }
	else { 2_u128.pow(idx as u32) }
}



#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn t_rip_flags() {
		// Make sure our flags are unique.
		let mut all = vec![
			FLAG_C2,
			FLAG_CACHE_BUST,
			FLAG_RAW,
			FLAG_RECONFIRM,
			FLAG_TRUST,
		];
		all.sort_unstable();
		all.dedup();
		assert_eq!(all.len(), 5);
	}

	#[test]
	fn t_rip_options_c2() {
		for v in [false, true] {
			let opts = RipOptions::default().with_c2(v);
			assert_eq!(opts.c2(), v);
		}
	}

	#[test]
	fn t_rip_options_cache_bust() {
		for v in [false, true] {
			let opts = RipOptions::default().with_cache_bust(v);
			assert_eq!(opts.cache_bust(), v);
		}
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
	fn t_rip_options_paranoia() {
		for v in [1, 2, 3] {
			let opts = RipOptions::default().with_paranoia(v);
			assert_eq!(opts.paranoia(), v);
		}

		// Min.
		let opts = RipOptions::default().with_paranoia(0);
		assert_eq!(opts.paranoia(), 1);

		// Max.
		let opts = RipOptions::default().with_paranoia(64);
		assert_eq!(opts.paranoia(), 32);
	}

	#[test]
	fn t_rip_options_raw() {
		for v in [false, true] {
			let opts = RipOptions::default().with_raw(v);
			assert_eq!(opts.raw(), v);
		}
	}

	#[test]
	fn t_rip_options_reconfirm() {
		for v in [false, true] {
			let opts = RipOptions::default().with_reconfirm(v);
			assert_eq!(opts.reconfirm(), v);
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
		assert_eq!(opts.refine(), 15);
		assert_eq!(opts.passes(), 16);
	}

	#[test]
	fn t_rip_options_tracks() {
		let mut opts = RipOptions::default();
		assert!(! opts.has_tracks(), "The track list should be empty!");

		// Add all possible tracks.
		for idx in 0..=u8::MAX { opts = opts.with_track(idx); }
		assert!(opts.has_tracks(), "The track list should not be empty!");

		// Pull them back.
		let tracks = opts.tracks().collect::<Vec<u8>>();
		assert_eq!(tracks.len(), 99, "Expected 99 tracks.");

		// Make sure everything is where we expect it to be.
		for (real, expected) in tracks.into_iter().zip(1..=99_u8) {
			assert_eq!(real, expected, "Options track mismatch: {real} instead of {expected}.");
		}
	}

	#[test]
	fn t_rip_options_trust() {
		for v in [false, true] {
			let opts = RipOptions::default().with_trust(v);
			assert_eq!(opts.trust(), v);
		}
	}
}
