/*!
# Rip Rip Hooray: Barcodes
*/

use crate::{
	macros::log,
	RipRipError,
};
use std::fmt;
use trimothy::TrimSliceMatches;



#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
/// # Barcode.
///
/// This is a simple wrapper for UPC/EAN barcodes that enforces validity and
/// consistent formatting.
pub struct Barcode([u8; 13]);

impl fmt::Display for Barcode {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		/// # Slice Writer.
		///
		/// This struct helps avoid the UTF8 and bounds-related overhead we'd
		/// face from converting the data to string slices.
		struct SliceFmt<'a>(&'a [u8]);

		impl fmt::Display for SliceFmt<'_> {
			fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				use std::fmt::Write;
				for c in self.0.iter().copied() { f.write_char(char::from(c))?; }
				Ok(())
			}
		}

		// Alternate is digits-only.
		if f.alternate() {
			write!(f, "{}", SliceFmt(self.0.as_slice()))
		}
		// Treat like UPC12 if the first digit is zero.
		else if self.0[0] == b'0' {
			write!(
				f,
				"{}-{}-{}-{}",
				SliceFmt(&self.0[1..2]),
				SliceFmt(&self.0[2..7]),
				SliceFmt(&self.0[7..12]),
				SliceFmt(&self.0[12..]),
			)
		}
		// Otherwise like an EAN13.
		else {
			write!(
				f,
				"{}-{}-{}",
				SliceFmt(&self.0[..1]),
				SliceFmt(&self.0[1..7]),
				SliceFmt(&self.0[7..]),
			)
		}
	}
}

impl TryFrom<&[u8]> for Barcode {
	type Error = RipRipError;
	fn try_from(mut src: &[u8]) -> Result<Self, Self::Error> {
		// Remove whitespace, leading *ASCII* zeroes, and trailing nulls.
		src = src.trim_start_matches(|b: u8| b.is_ascii_whitespace() || b == b'0');
		src = src.trim_end_matches(|b: u8| b.is_ascii_whitespace() || b == 0);

		// If there's a null byte, cut to it and recurse.
		if let Some(pos) = src.iter().copied().position(|b| b == 0_u8) {
			return Self::try_from(&src[..pos]);
		}

		// If there are dashes, strip and recurse.
		if src.contains(&b'-') {
			let new: Vec<u8> = src.iter()
				.copied()
				.filter(u8::is_ascii_digit)
				.collect();
			return Self::try_from(new.as_slice());
		}

		// Make sure we've got 8-13 ASCII digits and nothing else.
		if ! (8..=13).contains(&src.len()) || ! src.iter().all(u8::is_ascii_digit) {
			if ! src.is_empty() {
				log!(@trace "Invalid barcode {:?}.", src);
			}
			return Err(RipRipError::Barcode);
		}

		// Copy the data to the end of an ASCII-zero-padded slice.
		let mut buf = [b'0'; 13];
		buf[13 - src.len()..].copy_from_slice(src);

		// Return it if valid!
		if is_ean13(&buf) { Ok(Self(buf)) }
		else {
			if buf != [b'0'; 13] {
				log!(@trace [ buf ] "Invalid barcode {src:?}.");
			}
			Err(RipRipError::Barcode)
		}
	}
}

impl TryFrom<&str> for Barcode {
	type Error = RipRipError;

	#[inline]
	fn try_from(src: &str) -> Result<Self, Self::Error> {
		Self::try_from(src.as_bytes())
	}
}

impl Barcode {
	#[must_use]
	/// # From Raw Subchannel Packet.
	///
	/// Parse and return a barcode from a raw sub-q data payload, if valid.
	///
	/// Bytes `0..=6` hold the BCD-encoded data; the remainder is irrelevant
	/// for our purposes.
	pub(crate) fn from_subq(raw: [u8; 9]) -> Option<Self> {
		// Unpack the BCD.
		let inner: [u8; 13] = std::array::from_fn(|i| {
			let byte = raw[i / 2];
			b'0' + if i % 2 == 0 { byte >> 4 } else { byte & 0x0f }
		});

		if is_ean13(&inner) { Some(Self(inner)) }
		else {
			if inner != [b'0'; 13] { log!(@trace "Invalid barcode {inner:?}."); }
			None
		}
	}
}



#[must_use]
/// # Is EAN13?
///
/// The content is pre-validated by the `TryFrom` implementation; this merely
/// performs the computations to verify the check digit matches.
const fn is_ean13(src: &[u8; 13]) -> bool {
	// Total the digits (as decimals) using an alternating 1-or-3 multiplier.
	let mut total = 0;
	let mut k = 0;
	while k < 12 {
		let num = src[k as usize] ^ b'0';
		total += ((k % 2) * 2 + 1) * num;
		k += 1;
	}

	// The last digit is the check.
    let chk =
		if src[12] == b'0' { 10 }
		else { src[12] ^ b'0' };

	// Contains at least one non-zero digit and…
	(total != 0 || chk != 10) &&
	// Check digit matches.
	(10 - (total % 10) == chk)
}



#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn t_is_ean13() {
		assert!(is_ean13(b"0008811126827"));
		assert!(is_ean13(b"0018861006529"));
		assert!(is_ean13(b"0042282848420"));
		assert!(is_ean13(b"0075597996524"));
		assert!(is_ean13(b"0075992742320"));
		assert!(! is_ean13(b"0089218545555"));
		assert!(is_ean13(b"0089218545992"));
		assert!(is_ean13(b"0731455829921"));
		assert!(! is_ean13(b"0732455829921"));
		assert!(is_ean13(b"0886977200922"));
		assert!(is_ean13(b"5099997200628"));
		assert!(is_ean13(b"9332727016318"));

		// Test formatting too.
		let bc = Barcode::try_from("9332727016318").expect("Barcode failed.");
		assert_eq!(bc.to_string(), "9-332727-016318");

		let bc = Barcode::try_from("0018861006529").expect("Barcode failed.");
		assert_eq!(bc.to_string(), "0-18861-00652-9");

		// No matte codes should be valid.
		let mut buf = [0_u8; 13];
		for i in 0..=9_u8 {
			buf.fill(i);
			assert!(! is_ean13(&buf));
		}
	}
}
