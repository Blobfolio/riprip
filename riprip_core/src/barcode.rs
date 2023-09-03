/*!
# Rip Rip Hooray: Barcodes
*/

use crate::RipRipError;
use std::fmt;
use trimothy::TrimSliceMatches;



#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
/// # Barcode.
///
/// This is a simple wrapper for UPC/EAN barcodes that ensures validity and
/// consistent formatting.
pub struct Barcode([u8; 13]);

impl fmt::Display for Barcode {
	#[allow(unsafe_code)]
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		// Safety: all values are ASCII digits.
		let s = unsafe { std::str::from_utf8_unchecked(self.0.as_slice()) };

		// Treat like UPC12 if the first digit is zero.
		if self.0[0] == b'0' {
			write!(
				f,
				"{}-{}-{}-{}",
				&s[1..2],
				&s[2..7],
				&s[7..12],
				&s[12..],
			)
		}
		// Otherwise like an EAN13.
		else {
			write!(
				f,
				"{}-{}-{}",
				&s[..1],
				&s[1..7],
				&s[7..],
			)
		}
	}
}

impl TryFrom<&[u8]> for Barcode {
	type Error = RipRipError;
	fn try_from(mut src: &[u8]) -> Result<Self, Self::Error> {
		// Remove whitespace, leading *ASCII* zeroes, and trailing nulls.
		src = src.trim_start_matches(|b| b.is_ascii_whitespace() || b == b'0');
		src = src.trim_end_matches(|b| b.is_ascii_whitespace() || b == 0);

		// Make sure we've got 8-13 ASCII digits and nothing else.
		if ! (8..=13).contains(&src.len()) || ! src.iter().all(u8::is_ascii_digit) {
			return Err(RipRipError::Barcode);
		}

		// Move the data into place, zero-padding the left as needed so that it
		// occupies thirteen bytes.
		let mut maybe = [b'0'; 13];
		maybe[13 - src.len()..].copy_from_slice(src);

		// Make sure it validates.
		if is_ean13(&maybe) { Ok(Self(maybe)) }
		else { Err(RipRipError::Barcode) }
	}
}

impl TryFrom<&str> for Barcode {
	type Error = RipRipError;

	#[inline]
	fn try_from(src: &str) -> Result<Self, Self::Error> {
		Self::try_from(src.as_bytes())
	}
}



/// # Is EAN13?
fn is_ean13(src: &[u8; 13]) -> bool {
	let mut chk = 0;
	let mut total = 0;
	let mut k = 13;
	for num in src.iter().copied().rev() {
		k -= 1;

		// Convert ASCII to decimal. (TryFrom verifies all values are digits.)
		let num = num ^ b'0';

		// The last entry (the first we're checking) is the check digit.
		if k == 12 {
			if num == 0 { chk = 10; }
			else { chk = num; }
		}
		// Everything else goes into the total.
		else { total += ((k % 2) * 2 + 1) * num; }

	}

	10 - (total % 10) == chk
}



#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn t_is_ean13() {
		assert_eq!(is_ean13(&*b"0008811126827"), true);
		assert_eq!(is_ean13(&*b"0018861006529"), true);
		assert_eq!(is_ean13(&*b"0042282848420"), true);
		assert_eq!(is_ean13(&*b"0075597996524"), true);
		assert_eq!(is_ean13(&*b"0075992742320"), true);
		assert_eq!(is_ean13(&*b"0089218545555"), false);
		assert_eq!(is_ean13(&*b"0089218545992"), true);
		assert_eq!(is_ean13(&*b"0731455829921"), true);
		assert_eq!(is_ean13(&*b"0732455829921"), false);
		assert_eq!(is_ean13(&*b"0886977200922"), true);
		assert_eq!(is_ean13(&*b"5099997200628"), true);
		assert_eq!(is_ean13(&*b"9332727016318"), true);

		// Test formatting too.
		let bc = Barcode::try_from("9332727016318").expect("Barcode failed.");
		assert_eq!(bc.to_string(), "9-332727-016318");

		let bc = Barcode::try_from("0018861006529").expect("Barcode failed.");
		assert_eq!(bc.to_string(), "0-18861-00652-9");
	}
}
