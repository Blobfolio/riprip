/*!
# Rip Rip Hooray: ISRC.
*/

use crate::{
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

	fn try_from(src: &[u8]) -> Result<Self, Self::Error> {
		use trimothy::TrimSliceMatches;

		/// # Parse.
		fn parse(src: &[u8]) -> Option<[u8; 12]> {
			let mut out = [b'0'; 12];
			let mut dst = out.iter_mut();

			for b in src.iter().copied() {
				match b {
					// Silently ignore whitespace and dashes.
					b'\t' | b'\n' | b'\x0C' | b'\r' | b' ' | b'-' => {},

					// Break on null.
					b'\0' => break,

					// Write ASCII alphanumerics.
					b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' => {
						let v = dst.next()?;
						*v = b.to_ascii_uppercase();
					},

					// Anything else is an error.
					_ => return None,
				}
			}

			// If we've used up all the slots and the bytes have the right
			// composition, return it!
			if dst.next().is_none() && valid_bytes(out) { Some(out) }
			else { None }
		}

		// Return it if valid!
		parse(src.trim_matches(|b: u8| b.is_ascii_whitespace() || b == 0_u8)).map_or_else(
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
	/// Parse and return an ISRC from a raw sub-q data payload, if valid.
	///
	/// This is some monstrous bit-packed nonsense:
	/// * Country Code (6 + 6)
	/// * Owner Code (6 + 6 + 6)
	/// * Padding (2)
	/// * Year, BCD (8)
	/// * Serial, BCD (5 + 5 + 5 + 5)
	pub(crate) fn from_subq(raw: [u8; 9]) -> Option<Self> {
		/// # Convert BCD.
		const fn from_bcd8(v: u8) -> u32 {
			let v = v as u32;
			(v & 0x0F) + ((v >> 4) * 10)
		}

		// Pull the year since it's some binary decimal bullshit, and make
		// sure it doesn't exceed two digits.
		let year = from_bcd8(raw[4]);
		if 99 < year {
			log!(
				@trace [raw]
				"ISRC subchannel packet contained invalid year: {year}."
			);
			return None;
		}

		// Same for the serial, but in this case the max is five digits.
		let mut serial =
			from_bcd8(raw[5]) * 1_000 +
			from_bcd8(raw[6]) * 10 +
			u32::from(raw[7] >> 4);
		if 99_999 < serial {
			log!(
				@trace [raw]
				"ISRC subchannel packet contained invalid serial: {serial}."
			);
			return None;
		}

		// Convert the serial to zero-padded ASCII digits.
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
			(0x30 + (raw[0] >> 2)).to_ascii_uppercase(),
			(0x30 + (((raw[0] & 0x03) << 4) | (raw[1] >> 4))).to_ascii_uppercase(),
			(0x30 + (((raw[1] & 0x0f) << 2) | (raw[2] >> 6))).to_ascii_uppercase(),
			(0x30 + (raw[2] & 0x3f)).to_ascii_uppercase(),
			(0x30 + (raw[3] >> 2)).to_ascii_uppercase(),
			(year as u8 / 10) + b'0',
			(year as u8 % 10) + b'0',
			s1 as u8 + b'0',
			s2 as u8 + b'0',
			s3 as u8 + b'0',
			s4 as u8 + b'0',
			serial as u8 + b'0',
		];

		// The year and serial parts should be fine, but let's make sure the
		// country and owner character makeup is correct.
		if valid_bytes(inner) { Some(Self(inner)) }
		else {
			std::hint::cold_path();
			log!(@trace "Invalid ISRC {inner:?}.");
			None
		}
	}
}



#[must_use]
/// # Valid Inner.
///
/// This checks the inner ISRC array contains valid characters at each point:
const fn valid_bytes(raw: [u8; 12]) -> bool {
	matches!(
		raw,
		[
			              b'A'..=b'Z', // Country code.
			              b'A'..=b'Z', // Country code.
			b'0'..=b'9' | b'A'..=b'Z', // Owner code.
			b'0'..=b'9' | b'A'..=b'Z', // Owner code.
			b'0'..=b'9' | b'A'..=b'Z', // Owner code.
			b'0'..=b'9',               // Year.
			b'0'..=b'9',               // Year.
			b'0'..=b'9',               // Serial number.
			b'0'..=b'9',               // Serial number.
			b'0'..=b'9',               // Serial number.
			b'0'..=b'9',               // Serial number.
			b'0'..=b'9',               // Serial number.
		]
	)
}



#[cfg(test)]
mod tests {
	use super::*;
	use crate::SubQ;

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
			let Some(SubQ::Isrc(chopped)) = SubQ::new(&raw) else {
				panic!("Failed to validate raw packet {raw:?}");
			};
			let Some(parsed) = Isrc::from_subq(chopped) else {
				panic!("Failed to parse raw packet {chopped:?}");
			};
			assert_eq!(parsed.to_string(), expected);
		}

		// If we fuck with the checksum, it should fail.
		assert!(
			SubQ::new(&[
				3, 81, 85, 194, 36, 150, 6, 80, 16, 7, 200, 200, 0, 0, 0, 0
			]).is_none()
		);

		// Or leave the checksum and fuck with the value.
		assert!(
			SubQ::new(&[
				3, 80, 85, 194, 36, 150, 6, 80, 16, 7, 238, 125, 0, 0, 0, 0
			]).is_none()
		);
	}
}
