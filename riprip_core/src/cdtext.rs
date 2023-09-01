/*!
# Rip Rip Hooray: CDText.
*/

use std::{
	cmp::Ordering,
	fmt,
};



#[repr(u32)]
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
/// # CDText Field.
pub enum CDTextKind {
	Arranger = libcdio_sys::cdtext_field_t_CDTEXT_FIELD_ARRANGER,
	Barcode = libcdio_sys::cdtext_field_t_CDTEXT_FIELD_UPC_EAN,
	Composer = libcdio_sys::cdtext_field_t_CDTEXT_FIELD_COMPOSER,
	Isrc = libcdio_sys::cdtext_field_t_CDTEXT_FIELD_ISRC,
	Message = libcdio_sys::cdtext_field_t_CDTEXT_FIELD_MESSAGE,
	Performer = libcdio_sys::cdtext_field_t_CDTEXT_FIELD_PERFORMER,
	Songwriter = libcdio_sys::cdtext_field_t_CDTEXT_FIELD_SONGWRITER,
	Title = libcdio_sys::cdtext_field_t_CDTEXT_FIELD_TITLE,
}

impl AsRef<str> for CDTextKind {
	#[inline]
	fn as_ref(&self) -> &str { self.as_str() }
}

impl fmt::Display for CDTextKind {
	#[inline]
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(self.as_str())
	}
}

impl Ord for CDTextKind {
	#[inline]
	fn cmp(&self, rhs: &Self) -> Ordering { self.as_str().cmp(rhs.as_str()) }
}

impl PartialOrd for CDTextKind {
	#[inline]
	fn partial_cmp(&self, rhs: &Self) -> Option<Ordering> { Some(self.cmp(rhs)) }
}

impl CDTextKind {
	#[must_use]
	/// # As Str.
	pub const fn as_str(self) -> &'static str {
		match self {
			Self::Arranger => "ARRANGER",
			Self::Barcode => "BARCODE",
			Self::Composer => "COMPOSER",
			Self::Isrc => "ISRC",
			Self::Message => "COMMENT",
			Self::Performer => "ARTIST",
			Self::Songwriter => "SONGWRITER",
			Self::Title => "TITLE",
		}
	}
}
