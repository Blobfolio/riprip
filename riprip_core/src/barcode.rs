/*!
# Rip Rip Hooray: Barcodes
*/

use crate::{
	CRC,
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
			log!(@trace [ buf ] "Invalid barcode {src:?}.");
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
	/// Verify the packet, and if MCN, parse and return the value.
	///
	/// The first byte holds the control and ADR codes. ADR-2 is MCN.
	///
	/// Bytes `1..=7` hold the BCD-encoded data.
	///
	/// A 2-byte CRC for the first ten bytes is stored in bytes 11-12, big
	/// endian.
	///
	/// The remainder is irrelevant for our purposes.
	pub(crate) fn from_subchannel_packet(raw: &[u8; 16]) -> Option<Self> {
		// Short-circuit: only ADR-3 is relevant.
		if 2 != raw[0] & 0b0000_1111 { return None; }

		// Check the data first.
		let chk_actual = chk10(raw);
		let chk_expected = u16::from_be_bytes([raw[10], raw[11]]);
		if chk_actual != chk_expected {
			log!(
				@trace [chk_actual, chk_expected, raw]
				"MCN subchannel packet failed CRC verification.",
			);
			return None;
		}

		let inner: [u8; 13] = std::array::from_fn(|i| {
			let byte = raw[1 + i / 2];
			b'0' + if i % 2 == 0 { byte >> 4 } else { byte & 0x0f }
		});

		if is_ean13(&inner) { Some(Self(inner)) }
		else {
			std::hint::cold_path();
			log!(@trace "Invalid barcode {inner:?}.");
			None
		}
	}
}



#[must_use]
/// # Checksum 10 Bytes.
const fn chk10(raw: &[u8; 16]) -> u16 {
	let mut crc = 0_u16;
	let mut i = 0;
	while i < 10 {
		let idx = (((crc >> 8) ^ (raw[i] as u16)) & 0xFF) as usize;
		crc = CRC[idx] ^ (crc << 8);
		i += 1;
	}
	crc ^ 0xFFFF
}

/// # Is EAN13?
///
/// The content is pre-validated by the `TryFrom` implementation; this merely
/// performs the computations to verify the check digit matches.
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
	}
}
