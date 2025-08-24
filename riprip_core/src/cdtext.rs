/*!
# Rip Rip Hooray: CDText.
*/

use std::{
	cmp::Ordering,
	fmt,
};



/// # Helper: CDText Fields.
macro_rules! fields {
	( $( $k:ident $v:ident $vstr:literal ),+ $(,)? ) => (
		#[repr(u32)]
		#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
		/// # CDText Field.
		///
		/// This enum simply rearranges the constants exported from `libcdio` in a
		/// friendlier format.
		pub enum CDTextKind {
			$(
				#[doc = concat!("# ", stringify!($k), ".")]
				$k = libcdio_sys::$v,
			)+
		}

		impl CDTextKind {
			#[must_use]
			/// # As Str.
			///
			/// Return the field as an uppercase string, similar to how it would
			/// appear in track metadata.
			pub const fn as_str(self) -> &'static str {
				match self {
					$( Self::$k => $vstr, )+
				}
			}
		}
	);
}

fields! {
	Arranger   cdtext_field_t_CDTEXT_FIELD_ARRANGER   "ARRANGER",
	Barcode    cdtext_field_t_CDTEXT_FIELD_UPC_EAN    "BARCODE",
	Composer   cdtext_field_t_CDTEXT_FIELD_COMPOSER   "COMPOSER",
	Isrc       cdtext_field_t_CDTEXT_FIELD_ISRC       "ISRC",
	Message    cdtext_field_t_CDTEXT_FIELD_MESSAGE    "COMMENT",
	Performer  cdtext_field_t_CDTEXT_FIELD_PERFORMER  "ARTIST",
	Songwriter cdtext_field_t_CDTEXT_FIELD_SONGWRITER "SONGWRITER",
	Title      cdtext_field_t_CDTEXT_FIELD_TITLE      "TITLE",
}

impl AsRef<str> for CDTextKind {
	#[inline]
	fn as_ref(&self) -> &str { self.as_str() }
}

impl fmt::Display for CDTextKind {
	#[inline]
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		<str as fmt::Display>::fmt(self.as_str(), f)
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
