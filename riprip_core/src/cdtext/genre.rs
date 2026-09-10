/*!
# Rip Rip Hooray: CD-Text Genre.
*/

use std::fmt;



/// # Helper: Genre Code.
macro_rules! genre_code {
	( $( $k:ident $v:literal $str:literal, )+ ) => (
		#[repr(u8)]
		#[derive(Debug, Clone, Copy, Eq, PartialEq)]
		/// # CD-Text Genre Code.
		///
		/// Music that doesn't fit neatly into one of these predefined
		/// categories can optionally specify its genre as freeform text.
		pub(super) enum GenreCode {
			$(
				#[doc = concat!("# ", $str, ".")]
				$k = $v,
			)+
		}

		/// # Sanity Checks.
		const _: () = {
			let mut all: &[GenreCode] = &[$( GenreCode::$k, )+];
			assert!(all[0] as u8 == 0, "BUG: First GenreCode variant must be zero!");

			// Languages are sequential and contiguous.
			while let [ next, rest @ .. ] = all {
				assert!(
					rest.is_empty() || (*next as u8) + 1 == (rest[0] as u8),
					"BUG: GenreCodes are not incremental!",
				);
				all = rest;
			}
		};

		impl Default for GenreCode {
			#[inline]
			fn default() -> Self { Self::Unused }
		}

		impl fmt::Display for GenreCode {
			#[inline]
			fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.write_str(self.as_str())
			}
		}

		impl GenreCode {
			#[must_use]
			/// # From `u8`.
			pub(super) const fn from_u8(raw: u8) -> Option<Self> {
				match raw {
					$( $v => Some(Self::$k), )+
					_ => None,
				}
			}

			#[must_use]
			/// # As String Slice.
			const fn as_str(self) -> &'static str {
				match self {
					$( Self::$k => $str, )+
				}
			}

			#[must_use]
			/// # Is Genre?
			///
			/// Returns `true` if not unused or undefined.
			pub(super) const fn is_some(self) -> bool {
				! matches!(self, Self::Unused | Self::Unspecified)
			}

			#[must_use]
			/// # Split Raw Value.
			///
			/// CD-Text GENRE values store the code in the first byte and
			/// a freeform representation in the rest. Both parts are optional.
			pub(super) fn split_raw(raw: &[u8]) -> (Self, &str) {
				if let [ code, rest @ .. ] = raw {
					let code = Self::from_u8(*code).unwrap_or(Self::Unused);
					let rest = std::str::from_utf8(rest).map_or("", str::trim);
					(code, rest)
				}
				else { (Self::Unused, "") }
			}
		}
	);
}

genre_code! {
	Unused                0x00 "",
	Unspecified           0x01 "Unspecified",
	AdultContemporary     0x02 "Adult Contemporary",
	AlternativeRock       0x03 "Alternative Rock",
	Childrens             0x04 "Children's Music",
	Classical             0x05 "Classical",
	ChristianContemporary 0x06 "Christian Contemporary",
	Country               0x07 "Country",
	Dance                 0x08 "Dance",
	EasyListening         0x09 "Easy Listening",
	Erotic                0x0A "Erotic",
	Folk                  0x0B "Folk",
	Gospel                0x0C "Gospel",
	HipHop                0x0D "Hip-Hop",
	Jazz                  0x0E "Jazz",
	Latin                 0x0F "Latin",
	Musical               0x10 "Musical",
	NewAge                0x11 "New Age",
	Opera                 0x12 "Opera",
	Operetta              0x13 "Operetta",
	Pop                   0x14 "Pop",
	Rap                   0x15 "Rap",
	Reggae                0x16 "Reggae",
	Rock                  0x17 "Rock",
	RhythmAndBlues        0x18 "R&B",
	SoundEffects          0x19 "Sound Effects",
	// Note: the end of the list is inconsistent across libcdio; if possible,
	// try to find a CD matching any of these last three to confirm.
	Soundtrack            0x1A "Soundtrack",
	SpokenWord            0x1B "Spoken Word",
	World                 0x1C "World Music",
}
