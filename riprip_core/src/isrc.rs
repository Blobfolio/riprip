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

impl TryFrom<&[u8]> for Isrc {
	type Error = RipRipError;

	fn try_from(src: &[u8]) -> Result<Self, Self::Error> {
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

			// Return, unless we have unwritten slots left over!
			if dst.next().is_none() { Some(out) }
			else { None }
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
}
