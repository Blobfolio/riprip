/*!
# Rip Rip Hooray: Barcodes
*/

use crate::{
	macros::log,
	RipRipError,
};
use std::fmt;



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
	fn try_from(src: &[u8]) -> Result<Self, Self::Error> {
		use trimothy::TrimSliceMatches;

		/// # Parse.
		fn parse(src: &[u8]) -> Option<[u8; 13]> {
			// If there's a null in the middle somewhere, split and recurse.
			if let Some(pos) = src.iter().copied().position(|b| b == 0_u8) {
				std::hint::cold_path();
				return parse(&src[..pos]);
			}

			let mut out = [b'0'; 13];
			let mut dst = out.iter_mut().rev();

			// Write backwards.
			for b in src.trim_start_matches(|b: u8| b == b'0').iter().copied().rev() {
				match b {
					// Silently ignore whitespace and dashes.
					b'\t' | b'\n' | b'\x0C' | b'\r' | b' ' | b'-' => {},

					// Write ASCII digits.
					b'0'..=b'9' => {
						let v = dst.next()?;
						*v = b.to_ascii_uppercase();
					},

					// Anything else is an error.
					_ => return None,
				}
			}

			// Return if valid.
			if is_ean13(&out) { Some(out) }
			else { None }
		}

		// Return it if valid!
		parse(src.trim_matches(|b: u8| b.is_ascii_whitespace() || b == 0_u8)).map_or_else(
			|| {
				log!(@trace "Invalid UPC/EAN {:?}.", src);
				Err(RipRipError::Barcode)
			},
			|v| Ok(Self(v)),
		)
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
	let mut total = 0_u16;
	let mut k = 0_u16;
	while k < 12 {
		let num = (src[k as usize] ^ b'0') as u16;
		total += ((k % 2) * 2 + 1) * num;
		k += 1;
	}

	// The last digit is the check.
    let chk: u16 =
		if src[12] == b'0' { 10 }
		else { (src[12] ^ b'0') as u16 };

	// Contains at least one non-zero digit and…
	(total != 0 || chk != 10) &&
	// Check digit matches.
	(10 - (total % 10) == chk)
}



#[cfg(test)]
mod tests {
	use super::*;
	use crate::SubQ;

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

	#[test]
	fn t_barcode_q() {
		// A valid packet, but an invalid barcode.
		let Some(SubQ::Mcn(chopped)) = SubQ::new(&[2, 0, 117, 103, 146, 33, 50, 96, 0, 66, 166, 244, 0, 0, 0, 0]) else {
			panic!("Failed to parse raw sub-q packet.");
		};
		assert!(Barcode::from_subq(chopped).is_none());

		// Valid and valid.
		let Some(SubQ::Mcn(chopped)) = SubQ::new(&[2, 0, 117, 103, 0, 5, 82, 80, 0, 70, 190, 81, 0, 0, 0, 0]) else {
			panic!("Failed to parse raw sub-q packet.");
		};
		let barcode = Barcode::from_subq(chopped).unwrap();
		assert_eq!(barcode.to_string(), "0-75670-00552-5");
	}
}
