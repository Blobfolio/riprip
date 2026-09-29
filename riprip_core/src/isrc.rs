/*!
# Rip Rip Hooray: ISRC.
*/

use crate::{
	CRC,
	macros::log,
	RipRipError,
};
use dactyl::NoHash;
use std::{
	collections::HashMap,
	fmt,
};



/// # ISRC by Track.
pub type IsrcMap = HashMap<u8, Isrc, NoHash>;



#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
/// # ISRC.
///
/// This is a simple wrapper for ISRC values that enforces validity and
/// consistent formatting.
///
/// All values consist of the following:
/// * Country Code (2)
/// * Owner Code (3)
/// * Year (2)
/// * Serial Number (5)
pub struct Isrc([u8; 12]);

impl fmt::Display for Isrc {
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

		// Alternate has no dashes.
		if f.alternate() {
			write!(f, "{}", SliceFmt(self.0.as_slice()))
		}
		// Otherwise format it pretty.
		else {
			write!(
				f,
				"{}-{}-{}-{}",
				SliceFmt(&self.0[..2]),
				SliceFmt(&self.0[2..5]),
				SliceFmt(&self.0[5..7]),
				SliceFmt(&self.0[7..]),
			)
		}
	}
}

impl TryFrom<&[u8]> for Isrc {
	type Error = RipRipError;

	fn try_from(mut src: &[u8]) -> Result<Self, Self::Error> {
		use trimothy::TrimSliceMatches;

		/// # Parse.
		fn parse(src: &[u8]) -> Option<[u8; 12]> {
			let mut out = [b'0'; 12];
			let mut dst = out.iter_mut().enumerate();

			for b in src.iter().copied() {
				// Silently ignore whitespace, nulls, and dashes.
				if b.is_ascii_whitespace() || matches!(b, b'\0' | b'-') { continue; }

				// Pull the next slot.
				let (k, v) = dst.next()?;

				// Must be a number or, if one of the first five bytes, a
				// letter.
				if b.is_ascii_digit() || (k < 5 && b.is_ascii_alphabetic()) {
					*v = b.to_ascii_uppercase();
				}
				// Anything else is invalid.
				else { return None; }
			}

			// Return, unless we have unwritten slots left over or every byte
			// is the same!
			if dst.next().is_none() && out.array_windows::<2>().any(|[a, b]| *a != *b) {
				Some(out)
			}
			else { None }
		}

		// Trim whitespace and nulls.
		src = src.trim_matches(|b: u8| b.is_ascii_whitespace() || b == 0_u8);

		// If there's a null in the middle somewhere, cut to it and recurse.
		if let Some(pos) = src.iter().copied().position(|b| b == 0_u8) {
			return Self::try_from(&src[..pos]);
		}

		// Return it if valid!
		parse(src).map_or_else(
			|| {
				log!(@trace "Invalid ISRC {:?}.", src);
				Err(RipRipError::Isrc)
			},
			|v| Ok(Self(v)),
		)
	}
}

impl TryFrom<&str> for Isrc {
	type Error = RipRipError;

	#[inline]
	fn try_from(src: &str) -> Result<Self, Self::Error> {
		Self::try_from(src.as_bytes())
	}
}

impl Isrc {
	#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
	#[must_use]
	/// # From Raw Subchannel Packet.
	///
	/// Verify the packet, and if ISRC, parse and return the value.
	///
	/// This is monstrous.
	///
	/// The first byte holds the control and ADR codes. ADR-3 is ISRC.
	///
	/// Following that is some bit-packed nonsense:
	/// * Country Code (6 + 6)
	/// * Owner Code (6 + 6 + 6)
	/// * Padding (2)
	/// * Year, BCD (8)
	/// * Serial, BCD (5 + 5 + 5 + 5)
	///
	/// A 2-byte CRC for the first ten bytes is stored in bytes 11-12, big
	/// endian.
	///
	/// The remainder is irrelevant for our purposes.
	pub(crate) fn from_subchannel_packet(raw: &[u8; 16]) -> Option<Self> {
		/// # Convert BCD.
		const fn from_bcd8(v: u8) -> u32 {
			let v = v as u32;
			(v & 0x0F) + ((v >> 4) * 10)
		}

		// Short-circuit: only ADR-3 is relevant.
		if 3 != raw[0] & 0b0000_1111 { return None; }

		// Check the data first.
		let chk_actual = chk10(raw);
		let chk_expected = u16::from_be_bytes([raw[10], raw[11]]);
		if chk_actual != chk_expected {
			log!(
				@trace [chk_actual, chk_expected, raw]
				"ISRC subchannel packet failed CRC verification.",
			);
			return None;
		}

		// Pull the year since it's some binary decimal bullshit, and make
		// sure it doesn't exceed two digits.
		let year = from_bcd8(raw[5]);
		if 99 < year {
			log!(
				@trace [raw]
				"ISRC subchannel packet contained invalid year: {year}."
			);
			return None;
		}

		// Same for the serial, but in this case the max is five digits. // We checked it's 2 digits or less.
		let mut serial =
			from_bcd8(raw[6]) * 1_000 +
			from_bcd8(raw[7]) * 10 +
			u32::from(raw[8] >> 4);
		if 99_999 < serial {
			log!(
				@trace [raw]
				"ISRC subchannel packet contained invalid serial: {serial}."
			);
			return None;
		}

		// Convert the serial to ASCII digits.
		let s1 = serial / 10_000;
		serial -= s1 * 10_000;
		let s2 = serial / 1000;
		serial -= s2 * 1000;
		let s3 = serial / 100;
		serial -= s3 * 100;
		let s4 = serial / 10;
		serial -= s4 * 10;

		// Build the ISRC.
		let inner: [u8; 12] = [
			(0x30 + (raw[1] >> 2)).to_ascii_uppercase(),
			(0x30 + (((raw[1] & 0x03) << 4) | (raw[2] >> 4))).to_ascii_uppercase(),
			(0x30 + (((raw[2] & 0x0f) << 2) | (raw[3] >> 6))).to_ascii_uppercase(),
			(0x30 + (raw[3] & 0x3f)).to_ascii_uppercase(),
			(0x30 + (raw[4] >> 2)).to_ascii_uppercase(),
			(year as u8 / 10) + b'0',
			(year as u8 % 10) + b'0',
			s1 as u8 + b'0',
			s2 as u8 + b'0',
			s3 as u8 + b'0',
			s4 as u8 + b'0',
			serial as u8 + b'0',
		];

		// The year and serial parts should be fine, but let's make sure the
		// country and owner are ASCII alphanumeric.
		if
			inner[0..5].iter().all(u8::is_ascii_alphanumeric) &&
			inner.array_windows::<2>().any(|[a, b]| *a != *b)
		{
			Some(Self(inner))
		}
		else {
			std::hint::cold_path();
			log!(@trace "Invalid ISRC {inner:?}.");
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



#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn t_isrc() {
		for (lhs, rhs) in [
			("\0  USSM10109208  \0", Some("US-SM1-01-09208")),
			("US-SM1-99-01052", Some("US-SM1-99-01052")),
			("us-sm1-99-01052", Some("US-SM1-99-01052")),
			("XYBLG1101234", Some("XY-BLG-11-01234")),
			("null", None),
			("XYBLGX101234", None),
			("XYBLG1X01234", None),
			("XYBLG11X1234", None),
			("XYBLG110X234", None),
			("XYBLG1101X34", None),
			("XYBLG11012X4", None),
			("XYBLG110123X", None),
			("USSM101092089", None),
		] {
			match (Isrc::try_from(lhs).ok(), rhs) {
				(Some(a), Some(b)) => {
					assert_eq!(
						a.to_string(),
						b,
					);
				},
				(Some(a), None) => {
					panic!("ISRC {lhs:?} parsed as {a:?} instead of `None`.");
				},
				(None, Some(b)) => {
					panic!("ISRC {lhs:?} parsed as `None` instead of {b:?}.");
				},
				(None, None) => {},
			}
		}
	}

	#[test]
	fn t_isrc_q() {
		for (raw, expected) in [
			(
				[3, 81, 85, 194, 36, 150, 6, 80, 16, 7, 238, 125, 0, 0, 0, 0],
				"DE-G29-96-06501",
			),
			(
				[3, 81, 85, 194, 36, 150, 6, 80, 32, 35, 143, 14, 0, 0, 0, 0],
				"DE-G29-96-06502",
			),
			(
				[3, 81, 85, 194, 36, 150, 6, 80, 64, 40, 53, 79, 0, 0, 0, 0],
				"DE-G29-96-06504",
			),
			(
				[3, 81, 85, 194, 36, 150, 6, 80, 80, 35, 135, 87, 0, 0, 0, 0],
				"DE-G29-96-06505",
			),
			(
				[3, 81, 85, 194, 36, 150, 6, 81, 48, 102, 163, 44, 0, 0, 0, 0],
				"DE-G29-96-06513",
			),
		] {
			let Some(isrc) = Isrc::from_subchannel_packet(&raw) else {
				panic!("Failed to parse raw packet {raw:?}");
			};
			assert_eq!(isrc.to_string(), expected);
		}

		// If we fuck with the checksum, it should fail.
		assert!(
			Isrc::from_subchannel_packet(&[
				3, 81, 85, 194, 36, 150, 6, 80, 16, 7, 200, 200, 0, 0, 0, 0
			]).is_none()
		);

		// Or leave the checksum and fuck with the value.
		assert!(
			Isrc::from_subchannel_packet(&[
				3, 80, 85, 194, 36, 150, 6, 80, 16, 7, 238, 125, 0, 0, 0, 0
			]).is_none()
		);
	}
}
