/*!
# Rip Rip Hooray: Track Range.
*/

use std::range::legacy::RangeInclusive;



#[derive(Debug, Clone, Copy)]
/// # Track Range.
///
/// This struct holds an inclusive track range, sanity-checked for audio CD
/// contexts.
///
/// Note that in most cases, `cdtoc::Toc` is used instead.
pub(crate) struct TrackRange {
	/// # First Track.
	start: u8,

	/// # Last Track.
	end: u8,
}

impl Default for TrackRange {
	#[inline]
	fn default() -> Self {
		Self { start: 0, end: 0 }
	}
}

#[allow(
	clippy::allow_attributes,
	dead_code,
	reason = "Feature dependent.",
)]
impl TrackRange {
	/// # Max Track Number.
	pub(crate) const MAX: u8 = 99;

	#[must_use]
	/// # New.
	///
	/// Convert start/end track numbers into an inclusive range.
	pub(crate) const fn new(start: u8, end: u8) -> Option<Self> {
		if start <= end && 0 != end && end <= Self::MAX {
			Some(Self { start, end })
		}
		else { None }
	}

	#[must_use]
	/// # First Track.
	pub(crate) const fn first_track(self) -> u8 { self.start }

	#[must_use]
	/// # First Track.
	pub(crate) const fn last_track(self) -> u8 { self.end }

	#[must_use]
	/// # Length.
	pub(crate) const fn len(self) -> u8 { self.end + 1 - self.start }

	#[must_use]
	/// # As Range.
	pub(crate) const fn range(self) -> RangeInclusive<u8> { self.start..=self.end }
}



#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn t_track_range() {
		// Every combination up to and including MAX should be fine.
		for start in 0_u8..=TrackRange::MAX {
			for end in start..=TrackRange::MAX {
				// Special case; discs need more than HTOA.
				if start == 0 && end == 0 {
					assert!(TrackRange::new(start, end).is_none());
					continue;
				}

				let Some(rng) = TrackRange::new(start, end) else {
					panic!("Track range {start}..={end} failed.");
				};
				assert_eq!(rng.range(), start..=end);
				assert_eq!(rng.first_track(), start);
				assert_eq!(rng.last_track(), end);

				// End cannot be less than start.
				assert!(start == end || TrackRange::new(end, start).is_none());
			}
		}

		// A value bigger than MAX in either position should fail.
		assert!(TrackRange::new(0, TrackRange::MAX + 1).is_none());
		assert!(TrackRange::new(TrackRange::MAX + 1, TrackRange::MAX).is_none());
	}
}
