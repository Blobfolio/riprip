/*!
# Rip Rip Hooray: Read Offset
*/

use crate::{
	RipRipError,
	SAMPLES_PER_SECTOR,
};
use dactyl::traits::BytesToSigned;



/// # Min Offset.
const MIN_OFFSET: i16 = -5880;

/// # Max Offset.
const MAX_OFFSET: i16 = 5880;



#[derive(Debug, Clone, Copy, Default, Eq, Ord, PartialEq, PartialOrd)]
/// # Read Offset.
///
/// This holds a read offset in samples, but can return values in various other
/// useful formats.
///
/// For historical reasons, values are restricted to `-5880..=5880`.
pub struct ReadOffset(i16);

impl TryFrom<i16> for ReadOffset {
	type Error = RipRipError;
	fn try_from(src: i16) -> Result<Self, Self::Error> {
		if (MIN_OFFSET..=MAX_OFFSET).contains(&src) { Ok(Self(src)) }
		else { Err(RipRipError::ReadOffset) }
	}
}

impl TryFrom<&[u8]> for ReadOffset {
	type Error = RipRipError;
	fn try_from(src: &[u8]) -> Result<Self, Self::Error> {
		if src.is_empty() { Ok(Self(0)) }
		else {
			i16::btoi(src)
				.ok_or(RipRipError::ReadOffset)
				.and_then(Self::try_from)
		}
	}
}

impl TryFrom<&str> for ReadOffset {
	type Error = RipRipError;
	fn try_from(src: &str) -> Result<Self, Self::Error> {
		Self::try_from(src.as_bytes())
	}
}

impl ReadOffset {
	#[must_use]
	/// # Is Negative?
	pub const fn is_negative(self) -> bool { self.0 < 0 }

	#[must_use]
	/// # Samples.
	pub const fn samples(self) -> i16 { self.0 }

	#[must_use]
	/// # Samples (Absolute).
	pub const fn samples_abs(self) -> u16 { self.0.abs_diff(0) }
}

impl ReadOffset {
	#[must_use]
	#[allow(clippy::cast_possible_wrap)]
	/// # Sectors.
	///
	/// Return the minimum containing sector amount.
	pub const fn sectors(self) -> i16 {
		// Flip the sector count negative if needed.
		if self.is_negative() { 0 - self.sectors_abs() as i16 }
		else { self.sectors_abs() as i16 }
	}

	#[must_use]
	#[allow(
		clippy::cast_possible_truncation,
		clippy::integer_division,
	)]
	/// # Sectors (Absolute).
	///
	/// Return the minimum containing sector amount.
	///
	/// TODO: use div_ceil as soon as that is stabilized!
	pub const fn sectors_abs(self) -> u16 {
		if self.0 == 0 { return 0; }

		let samples_abs = self.samples_abs();

		// Floor.
		let div = samples_abs / SAMPLES_PER_SECTOR as u16;

		// Add one if there's a remainder.
		if 0 == samples_abs % SAMPLES_PER_SECTOR as u16 { div }
		else { div + 1 }
	}
}
