/*!
# Rip Rip Hooray: CD-Text Fields.
*/

use std::fmt;



/// # Helper: Logical Fields.
macro_rules! field {
	(
		$( #[doc = $doc:expr] )*
		$enum:ident
		$( $k:ident $str:literal $( $toc:literal )?, )+
	) => (
		#[repr(u8)]
		#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
		$( #[doc = $doc] )*
		pub enum $enum {
			$(
				#[doc = concat!("# ", $str, ".")]
				$k,
			)+
		}

		impl fmt::Display for $enum {
			#[inline]
			fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.write_str(self.as_str())
			}
		}

		impl $enum {
			/// # All Items.
			pub(crate) const ALL: [Self; crate::count!($( $k )+)] = [ $( Self::$k, )+ ];

			#[must_use]
			/// # As String Slice.
			pub const fn as_str(self) -> &'static str {
				match self {
					$( Self::$k => $str, )+
				}
			}

			#[must_use]
			/// # As String Slice for `.toc`.
			///
			/// This is very nearly the same as `as_str`, but with a few
			/// overrides specific to the `.toc` cuesheet format.
			pub const fn as_str_toc(self) -> &'static str {
				match self {
					$( $( Self::$k => $toc, )? )+
					_ => self.as_str(),
				}
			}
		}
	);
}

field! {
	/// # Disc Fields.
	///
	/// This enum holds the (logical) CD-Text fields applicable to the disc as
	/// a whole.
	DiscField
	Title      "TITLE",
	Performer  "PERFORMER",
	Songwriter "SONGWRITER",
	Composer   "COMPOSER",
	Arranger   "ARRANGER",
	Message    "MESSAGE",
	Barcode    "BARCODE"     "UPC_EAN",
	DiscId     "DISC ID"     "DISC_ID",
	Genre      "GENRE",
}
field! {
	/// # Track Fields.
	///
	/// This enum holds the (logical) CD-Text fields applicable to individual
	/// tracks on the disc.
	TrackField
	Title      "TITLE",
	Performer  "PERFORMER",
	Songwriter "SONGWRITER",
	Composer   "COMPOSER",
	Arranger   "ARRANGER",
	Message    "MESSAGE",
	Isrc       "ISRC",
}
