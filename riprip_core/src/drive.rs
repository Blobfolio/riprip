/*!
# Rip Rip Hooray: Drive Vendors, Models, and Read Offsets
*/

use crate::{
	RipRipError,
	SAMPLES_PER_SECTOR,
};
use dactyl::traits::BytesToSigned;
use std::{
	fmt,
	ops::RangeInclusive,
};
use trimothy::NormalizeWhitespace;



#[allow(clippy::cast_possible_wrap)] // It fits.
/// # Offset Range.
///
/// Ranges outside the ignorable regions in the AccurateRip/CTDB algorithms
/// don't make any practical sense.
const OFFSET_RNG: RangeInclusive<i16> =
	SAMPLES_PER_SECTOR as i16 * -5..=
	SAMPLES_PER_SECTOR as i16 * 5;

/// # Max Drive Vendor Length.
const DRIVE_VENDOR_LEN: usize = 8;

/// # Max Drive Model Length.
const DRIVE_MODEL_LEN: usize = 16;

// The data generated by build.rs. It is a constant array of known
// (DriveVendorModel, ReadOffset) pairs, and another of known cache sizes.
include!(concat!(env!("OUT_DIR"), "/drives.rs"));



#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
/// # Drive Vendor/Model.
///
/// Hardware vendor and model identifiers have hard limits of 8 and 16 bytes
/// respectively. By storing them together in a fixed 24-byte array, we can
/// make their values `Copy` while also improving search-by-pair efficiency.
///
/// While probably not strictly necessary, values are stored UPPERCASE to force
/// case insensitivity. They're also required to be ASCII.
///
/// Whitespace cannot be normalized at the point of storage because some pairs
/// differentiate themselves with spacing alone, but the `Display` impl cleans
/// up that nonsense.
pub struct DriveVendorModel([u8; 24]);

impl fmt::Display for DriveVendorModel {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		if let Ok(raw) = std::str::from_utf8(&self.0) {
			for c in raw.chars().normalized_control_and_whitespace() {
				write!(f, "{c}")?;
			}
		}

		Ok(())
	}
}

impl DriveVendorModel {
	/// # New!
	///
	/// Validate and parse separate vendor and model strings into our special
	/// model.
	///
	/// ## Errors
	///
	/// This will return an error if the lengths are out of range, or the
	/// model number is missing.
	pub(crate) fn new(mut vendor: &str, mut model: &str) -> Result<Self, RipRipError> {
		vendor = vendor.trim();
		model = model.trim();

		if DRIVE_VENDOR_LEN < vendor.len() || ! vendor.is_ascii() { Err(RipRipError::DriveVendor) }
		else if ! (1..=DRIVE_MODEL_LEN).contains(&model.len()) || ! model.is_ascii() {
			Err(RipRipError::DriveModel)
		}
		else {
			let mut buf = [0_u8; 24];
			for (b, v) in buf.iter_mut().zip(vendor.bytes()) {
				*b = v.to_ascii_uppercase();
			}
			for (b, v) in buf.iter_mut().skip(DRIVE_VENDOR_LEN).zip(model.bytes()) {
				*b = v.to_ascii_uppercase();
			}
			Ok(Self(buf))
		}
	}

	#[must_use]
	/// # Vendor.
	///
	/// Note: This may be empty.
	pub fn vendor(&self) -> &str {
		if self.0[0] == 0 { "" }
		else {
			let mut chunk = &self.0[..DRIVE_VENDOR_LEN];
			while let [ rest @ .., 0 ] = chunk { chunk = rest; }
			std::str::from_utf8(chunk).unwrap_or("")
		}
	}

	#[must_use]
	/// # Model.
	///
	/// A model number is always present.
	pub fn model(&self) -> &str {
		let mut chunk = &self.0[DRIVE_VENDOR_LEN..];
		while let [ rest @ .., 0 ] = chunk { chunk = rest; }
		std::str::from_utf8(chunk).unwrap_or("")
	}

	#[must_use]
	/// # Detect Cache Size.
	///
	/// If the vendor/model pair have a known cache size, the value is returned
	/// as a `u16`.
	pub fn detect_cache(&self) -> Option<u16> {
		let idx = DRIVE_CACHES.binary_search_by_key(self, |(k, _)| *k).ok()?;
		Some(DRIVE_CACHES[idx].1)
	}

	#[must_use]
	/// # Detect Offset.
	///
	/// If the vendor/model pair are known, return the drive offset.
	pub fn detect_offset(&self) -> Option<ReadOffset> {
		let idx = DRIVE_OFFSETS.binary_search_by_key(self, |(k, _)| *k).ok()?;
		Some(DRIVE_OFFSETS[idx].1)
	}
}



#[derive(Debug, Clone, Copy, Default, Eq, Ord, PartialEq, PartialOrd)]
/// # Read Offset.
///
/// This holds a read offset in samples, but can return values in various other
/// useful formats.
///
/// For functional reasons, values are restricted to `-2940..=2940`.
pub struct ReadOffset(i16);

impl TryFrom<i16> for ReadOffset {
	type Error = RipRipError;
	fn try_from(src: i16) -> Result<Self, Self::Error> {
		if OFFSET_RNG.contains(&src) { Ok(Self(src)) }
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
	/// # Sectors (Absolute).
	///
	/// Return the minimum containing sector amount.
	///
	/// TODO: use div_ceil as soon as that is stabilized!
	pub const fn sectors_abs(self) -> u16 {
		if self.0 == 0 { return 0; }

		let samples_abs = self.samples_abs();

		// Floor.
		let div = samples_abs.wrapping_div(SAMPLES_PER_SECTOR);

		// Add one if there's a remainder.
		if 0 == samples_abs % SAMPLES_PER_SECTOR { div }
		else { div + 1 }
	}
}



#[cfg(test)]
mod test {
	use super::*;


	#[test]
	fn t_vendormodel() {
		// Test some failures first.
		for (v, m) in [
			("", ""),
			("Foo", ""),
			("Immatoolongvendor", "Bar"),
			("Foo", "Immatoolongmodelnumber"),
		] {
			assert!(DriveVendorModel::new(v, m).is_err());
		}

		// Test things that should work.
		let vm = DriveVendorModel::new("\nPioneer ", "BD-RW   BDR-XD05   ")
			.expect("Unable to create DriveVendorModel.");
		assert_eq!(vm.vendor(), "PIONEER");
		assert_eq!(vm.model(), "BD-RW   BDR-XD05");
		assert_eq!(vm.to_string(), "PIONEER BD-RW BDR-XD05");
		assert_eq!(vm.detect_offset(), Some(ReadOffset(667)));
		assert_eq!(vm.detect_cache(), Some(4096));
	}

	#[test]
	fn t_offset() {
		for (raw, samples, samples_abs, sectors, sectors_abs) in [
			("0", 0_i16, 0_u16, 0_i16, 0_u16),
			("123", 123_i16, 123_u16, 1_i16, 1_u16),
			("-123", -123_i16, 123_u16, -1_i16, 1_u16),
			("588", 588_i16, 588_u16, 1_i16, 1_u16),
			("-588", -588_i16, 588_u16, -1_i16, 1_u16),
			("667", 667_i16, 667_u16, 2_i16, 2_u16),
			("-667", -667_i16, 667_u16, -2_i16, 2_u16),
		] {
			let offset = ReadOffset::try_from(raw).expect("ReadOffset failed.");
			assert_eq!(offset.samples(), samples);
			assert_eq!(offset.samples_abs(), samples_abs);
			assert_eq!(offset.sectors(), sectors);
			assert_eq!(offset.sectors_abs(), sectors_abs);
		}

		// Make sure the min and max both work, but no more.
		assert!(ReadOffset::try_from(*OFFSET_RNG.start()).is_ok());
		assert!(ReadOffset::try_from(*OFFSET_RNG.start() - 1).is_err());
		assert!(ReadOffset::try_from(*OFFSET_RNG.end()).is_ok());
		assert!(ReadOffset::try_from(*OFFSET_RNG.end() + 1).is_err());
	}
}
