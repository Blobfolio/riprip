/*!
# Rip Rip Hooray: Track Range.
*/

use std::range::legacy::RangeInclusive;
use super::CDTextError;



#[derive(Debug, Clone, Copy)]
/// # Track Range.
///
/// This struct holds an inclusive track range, sanity-checked for audio CD
/// contexts.
pub(super) struct TrackRange {
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

impl TrackRange {
	/// # Max Track Number.
	const MAX: u8 = 99;

	/// # New.
	///
	/// Convert start/end track numbers into an inclusive range.
	///
	/// ## Errors
	///
	/// This will return an error if `end` is less than `start`, or either
	/// are greater than `99`.
	pub(super) const fn new(start: u8, end: u8) -> Result<Self, CDTextError> {
		if start <= end && end <= Self::MAX {
			Ok(Self { start, end })
		}
		else { Err(CDTextError::InvalidTrackRange) }
	}

	/// # As Range.
	pub(super) const fn range(self) -> RangeInclusive<u8> { self.start..=self.end }
}



#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn t_track_range() {
		// Every combination up to and including MAX should be fine.
		for start in 0_u8..=TrackRange::MAX {
			for end in start..=TrackRange::MAX {
				let Ok(rng) = TrackRange::new(start, end) else {
					panic!("Track range {start}..={end} failed.");
				};
				assert_eq!(rng.range(), start..=end);

				// End cannot be less than start.
				assert!(start == end || TrackRange::new(end, start).is_err());
			}
		}

		// A value bigger than MAX in either position should fail.
		assert!(TrackRange::new(0, TrackRange::MAX + 1).is_err());
		assert!(TrackRange::new(TrackRange::MAX + 1, TrackRange::MAX).is_err());
	}
}
