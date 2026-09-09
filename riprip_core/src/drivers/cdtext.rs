/*!
# Rip Rip Hooray: CD-Text Parser

Resources:
<https://libcdio.github.io/cd-text-format.html>
<https://github.com/xbmc/libcdio/blob/master/example/cdtext.c>
*/

use crate::{
	Barcode,
	macros::log,
};
use dactyl::{
	NoHash,
	traits::NiceInflection,
};
use std::{
	collections::HashMap,
	fmt,
};



/// # Pack Length.
const PACK_LEN: usize = 18;

/// # Pack Header Length.
const PACK_HEADER_LEN: usize = 4;

/// # Pack Payload Length.
const PACK_PAYLOAD_LEN: usize = 12;

/// # Offset of Pack Checksum.
const PACK_CRC_OFFSET: usize = PACK_HEADER_LEN + PACK_PAYLOAD_LEN;

/// # Data Pack.
type Pack = [u8; PACK_LEN];

/// # Sanity Check.
const _: () = {
	assert!(
		PACK_LEN - PACK_CRC_OFFSET == 2,
		"BUG: CRC offset does not leave exactly two trailing bytes.",
	);
};



#[derive(Debug, Default)]
/// # CD-Text!
///
/// This struct holds CDText in all available languages.
pub struct CDText(Vec<CDTextInner>);

impl fmt::Display for CDText {
	#[inline]
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let mut any = false;
		for v in &self.0 {
			if any { writeln!(f)?; }
			else { any = true; }

			<CDTextInner as fmt::Display>::fmt(v, f)?;
		}
		Ok(())
	}
}

impl CDText {
	#[must_use]
	/// # Search Disc Value(s).
	///
	/// Returns an iterator of matching values across all provided
	/// languages.
	pub(crate) fn disc(&self, field: DiscField) -> CDTextDiscFieldIter<'_> {
		CDTextDiscFieldIter { field, set: &self.0 }
	}

	#[must_use]
	/// # Search Track Value(s).
	///
	/// Returns an iterator of matching values across all provided
	/// languages.
	pub(crate) fn track(&self, field: TrackField, track: u8)
	-> CDTextTrackFieldIter<'_> {
		CDTextTrackFieldIter { field, track, set: &self.0 }
	}
}

impl CDText {
	#[must_use]
	/// # Decode Raw Pack Data.
	pub(crate) fn from_bytes(pack_data: &[u8]) -> Option<Self> {
		let mut context = Context::default();
		let (chunks, remainder) = pack_data.as_chunks::<PACK_LEN>();

		// A single trailing \0 byte is a tolerated convention, otherwise
		// assume parsing is incomplete.
		if ! remainder.is_empty() && remainder != [0] {
			log!(
				@trace
				"CD-Text parsing ended with {}.",
				remainder.len().nice_inflect("byte", "bytes"),
			);
			return None;
		}

		// Parse the chunks.
		for pack in chunks {
			if ! chk_pack(pack) {
				log!(@trace "Invalid CD-Text pack {pack:?}.");
				return None;
			}
			if let Err(e) = context.parse_pack(pack) {
				log!(@trace "{e}");
				return None;
			}
		}

		// Build up the inner data.
		let mut inner = Vec::with_capacity(context.language_blocks.len());
		for (i, block) in context.language_blocks.into_iter().enumerate() {
			// Parse the size info.
			let size_info = block.buffer
				.get(&(PackKind::SizeInfo, 0))
				.map(Vec::as_slice)
				.ok_or(Error::MissingSizeInfo)
				.and_then(SizeInfo::try_from);
			let size_info = match size_info {
				Ok(v) => v,
				Err(e) => {
					log!(@trace "{e}");
					return None;
				},
			};

			// Check the counts match.
			if block.pack_count != size_info.total_expected_packs() {
				log!(@trace "Invalid CD-Text pack count.");
				return None;
			}

			let lang_code = size_info.lang_code[i];
			let char_code = size_info.char_code;
			let Some(language) = Language::from_u8(lang_code) else {
				log!(@trace "Invalid CD-Text language code: {lang_code}.");
				return None;
			};
			let Some(encoding) = Encoding::from_u8(char_code) else {
				log!(@trace "Invalid CD-Text encoding code: {char_code}.");
				return None;
			};
			let mut catalog = HashMap::default();
			for ((field, track), buf) in block.buffer {
				// Disc data?
				if track == 0 {
					if let Some(field) = field.disc_field() {
						let v =
							// Don't accept invalid barcodes.
							if matches!(field, DiscField::Barcode) {
								let Ok(v) = Barcode::try_from(buf.as_slice()) else {
									continue;
								};
								v.to_string()
							}
							else { encoding.decode(&buf) };
						catalog.insert(
							u16::from_le_bytes([field as u8, 0]),
							v,
						);
					}
				}
				// Track data?
				else if let Some(field) = field.track_field() {
					catalog.insert(
						u16::from_le_bytes([field as u8, track]),
						encoding.decode(&buf),
					);
				}
			}

			// Save it!
			inner.push(CDTextInner {
				first_track: size_info.first_track,
				last_track: size_info.last_track,
				language,
				catalog,
			});
		}

		// Done!
		if inner.is_empty() {
			std::hint::cold_path();
			None
		}
		else { Some(Self(inner)) }
	}
}



#[derive(Debug)]
/// # CD-Text `DiscField` Value Iterator.
///
/// This iterator yields all instances of `DiscField` across the various
/// languages.
pub(crate) struct CDTextDiscFieldIter<'a> {
	/// # Field of Interest.
	field: DiscField,

	/// # Language Data Sets.
	set: &'a [CDTextInner],
}

impl<'a> Iterator for CDTextDiscFieldIter<'a> {
	type Item = &'a str;

	fn next(&mut self) -> Option<Self::Item> {
		while let [ next, rest @ .. ] = &self.set {
			self.set = rest;
			if let Some(out) = next.disc(self.field) {
				return Some(out);
			}
		}

		None
	}
}

impl std::iter::FusedIterator for CDTextDiscFieldIter<'_> {}



#[derive(Debug)]
/// # CD-Text `TrackField` Value Iterator.
///
/// This iterator yields all instances of `DiscField` across the various
/// languages.
pub(crate) struct CDTextTrackFieldIter<'a> {
	/// # Field of Interest.
	field: TrackField,

	/// # Track Number.
	track: u8,

	/// # Language Data Sets.
	set: &'a [CDTextInner],
}

impl<'a> Iterator for CDTextTrackFieldIter<'a> {
	type Item = &'a str;

	fn next(&mut self) -> Option<Self::Item> {
		while let [ next, rest @ .. ] = &self.set {
			self.set = rest;
			if let Some(out) = next.track(self.field, self.track) {
				return Some(out);
			}
		}

		None
	}
}

impl std::iter::FusedIterator for CDTextTrackFieldIter<'_> {}



/// # Helper: Logical Fields.
macro_rules! field {
	(
		$( #[doc = $doc:expr] )*
		$enum:ident
		$( $k:ident $str:literal, )+
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
			const ALL: [Self; crate::count!($( $k )+)] = [ $( Self::$k, )+ ];

			#[must_use]
			/// # As String Slice.
			const fn as_str(self) -> &'static str {
				match self {
					$( Self::$k => $str, )+
				}
			}
		}
	);
}

field! {
	/// # Disc Fields.
	///
	/// This enum holds the (logical) CDText fields applicable to the disc as
	/// a whole.
	DiscField
	Title      "TITLE",
	Performer  "PERFORMER",
	Songwriter "SONGWRITER",
	Composer   "COMPOSER",
	Message    "MESSAGE",
	Arranger   "ARRANGER",
	Barcode    "BARCODE",
	DiscId     "DISC ID",
	Genre      "GENRE",
}
field! {
	/// # Track Fields.
	///
	/// This enum holds the (logical) CDText fields applicable to individual
	/// tracks on the disc.
	TrackField
	Title      "TITLE",
	Performer  "PERFORMER",
	Songwriter "SONGWRITER",
	Composer   "COMPOSER",
	Message    "MESSAGE",
	Arranger   "ARRANGER",
	Isrc       "ISRC",
}



#[derive(Debug, Clone, Default)]
/// # Raw CD-Text Block.
struct Block {
	/// # Pack Count.
	pack_count: usize,

	/// # Buffer.
	buffer: HashMap<(PackKind, u8), Vec<u8>>,
}



#[derive(Debug, Default)]
/// # CD-Text (Inner).
///
/// This struct holds disc and track CDText in a single language.
struct CDTextInner {
	/// # First Track.
	first_track: u8,

	/// # Last Track.
	last_track: u8,

	/// # Language.
	language: Language,

	/// # Data.
	catalog: HashMap<u16, String, NoHash>,
}

impl fmt::Display for CDTextInner {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		// Alternate formatting is used for the logger.
		if f.alternate() {
			// Language.
			write!(f, "\n  [{}]", self.language)?;

			// Disc fields first.
			f.write_str("\n  DISC:")?;
			for field in DiscField::ALL {
				let v = self.disc(field).map_or("", str::trim);
				if ! v.is_empty() {
					write!(f, "\n    {field}: {v}")?;
				}
			}
			if let Some(v) = self.genre_code() {
				write!(f, "\n    GENRE CODE: {} ({v})", v as u8)?;
			}

			// Track fields.
			for track in self.first_track..=self.last_track {
				write!(f, "\n  TRACK {track:02}:")?;
				for field in TrackField::ALL {
					let v = self.track(field, track).map_or("", str::trim);
					if ! v.is_empty() {
						write!(f, "\n    {field}: {v}")?;
					}
				}
			}
		}
		else {
			// Language.
			writeln!(f, "[{}]", self.language)?;

			// Disc fields first.
			f.write_str("DISC:\n")?;
			for field in DiscField::ALL {
				let v = self.disc(field).map_or("", str::trim);
				if ! v.is_empty() {
					writeln!(f, "\t{field}: {v}")?;
				}
			}
			if let Some(v) = self.genre_code() {
				writeln!(f, "\tGENRE CODE: {} ({v})", v as u8)?;
			}

			// Track fields.
			for track in self.first_track..=self.last_track {
				writeln!(f, "TRACK {track:02}:")?;
				for field in TrackField::ALL {
					let v = self.track(field, track).map_or("", str::trim);
					if ! v.is_empty() {
						writeln!(f, "\t{field}: {v}")?;
					}
				}
			}
		}

		Ok(())
	}
}

impl CDTextInner {
	#[must_use]
	/// # Disc Value.
	fn disc(&self, field: DiscField) -> Option<&str> {
		let raw = self.disc__(field)?;
		match field {
			// Album title might be prefixed with performer.
			DiscField::Title => self.disc__(DiscField::Performer)
				.and_then(|v| raw.strip_prefix(v))
				.map(str::trim_start)
				.or(Some(raw)),
			// Genre serves double duty.
			DiscField::Genre =>
				// The first byte is reserved for the genre code; the rest, if
				// any, is the freeform response.
				if 2 <= raw.len() && raw.bytes().next().is_some_and(|v| v.is_ascii()) {
					let v = raw[1..].trim();
					if v.is_empty() { None }
					else { Some(v) }
				}
				else { None },
			// Everything else is what it is.
			_ => Some(raw),
		}
	}

	#[must_use]
	/// # Raw Disc Value.
	fn disc__(&self, field: DiscField) -> Option<&str> {
		let v = self.catalog.get(&u16::from_le_bytes([field as u8, 0]))?.trim();
		if v.is_empty() { None }
		else { Some(v) }
	}

	#[must_use]
	/// # Track Value.
	fn track(&self, field: TrackField, track: u8) -> Option<&str> {
		if
			0 != track &&
			let Some(v) = self.catalog.get(&u16::from_le_bytes([field as u8, track]))
		{
			let v = v.trim();
			if v.is_empty() { None }
			else { Some(v) }
		}
		else { None }
	}

	#[must_use]
	/// # Genre Code.
	fn genre_code(&self) -> Option<GenreCode> {
		let genre = self.disc__(DiscField::Genre)?;

		// The first byte is the code.
		genre.bytes()
			.next()
			.and_then(GenreCode::from_u8)
	}
}



#[derive(Debug, Default)]
/// # Block Decoding Context.
struct Context {
	/// # Text Buffer.
	text_buf: Vec<u8>,

	/// # Language Blocks.
	language_blocks: Vec<Block>,
}

impl Context {
	/// # Parse Pack.
	fn parse_pack(&mut self, pack: &Pack) -> Result<(), Error> {
		let header = &pack[0..PACK_HEADER_LEN];
		let payload = &pack[PACK_HEADER_LEN..PACK_CRC_OFFSET];

		let (id1, id2, _, id4) = (header[0], header[1], header[2], header[3]);

		let is_extension = (id2 & 0x80) != 0; // Extension Flag (0 = normal, 1 = extension)
		if is_extension {
			return Err(Error::UnsupportedExtension);
		}
		let mut track_number = id2 & 0x7F;
		let block_id = (id4 >> 4) & 0x07; // Bits 4-6 define the language block ID.

		self.language_blocks
			.resize(block_id as usize + 1, Block::default());

		self.language_blocks[block_id as usize].pack_count += 1;

		let Some(field) = PackKind::from_u8(id1) else {
			// Safe early exit per CD-Text specification guidelines.
			return Ok(());
		};

		if field.is_data() {
			let key = (field, 0);
			let buffer = self.language_blocks[block_id as usize]
				.buffer
				.entry(key)
				.or_default();
			buffer.extend_from_slice(payload);
		}
		else {
			let is_double_byte = (id4 & 0x80) != 0;
			if is_double_byte {
				return Err(Error::UnsupportedDoubleByte);
			}
			for b in payload {
				if *b == 0x00 {
					if !self.text_buf.is_empty() {
						let key = (field, track_number);
						self.language_blocks[block_id as usize]
							.buffer
							.insert(key, self.text_buf.clone());
						self.text_buf.clear();
					}
					track_number += 1;
				}
				else if *b == b'\t' {
					// Handle repetition.
					let last_key = (field, track_number.saturating_sub(1));
					let cloned_buf = self.language_blocks[block_id as usize]
						.buffer
						.get(&last_key)
						.cloned();
					if let Some(buf) = cloned_buf {
						let key = (field, track_number);
						self.language_blocks[block_id as usize]
							.buffer
							.insert(key, buf);
					}
				}
				else {
					self.text_buf.push(*b);
				}
			}
		}
		Ok(())
	}
}



#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// # Encoding Kind.
enum Encoding {
	/// # ISO-8859-1 (8 bit), Latin-1.
	Iso8859_1 = 0x00,

	/// # ASCII (7 bit).
	Ascii = 0x01,

	/// # Shift-JIS (double byte).
	ShiftJis = 0x80,
}

impl Encoding {
	#[must_use]
	/// # From `u8`.
	const fn from_u8(raw: u8) -> Option<Self> {
		match raw {
			0x00 => Some(Self::Iso8859_1),
			0x01 => Some(Self::Ascii),
			0x80 => Some(Self::ShiftJis),
			_ => None,
		}
	}

	#[must_use]
	/// # Decode.
	///
	/// Parse a raw byte stream into a string, given the encoding.
	fn decode(self, bytes: &[u8]) -> String {
		match self {
			Self::Iso8859_1 | Self::Ascii => {
				// Try to parse directly as UTF-8/ASCII first without looping.
				std::str::from_utf8(bytes).map_or_else(
					|_| bytes.iter().map(|&b| b as char).collect(),
					std::borrow::ToOwned::to_owned,
				)
			}
			Self::ShiftJis => encoding_rs::SHIFT_JIS.decode(bytes).0.into_owned(),
		}
	}
}



/// # Helper: Error.
macro_rules! err {
	( $( $k:ident $v:literal, )+ ) => (
		#[derive(Debug, Clone, Copy)]
		/// # CDText Decoding Errors.
		///
		/// This enum serves as a cheap error type for CD-Text decoding.
		enum Error {
			$(
				#[doc = concat!("# ", $v)]
				$k,
			)+
		}

		impl fmt::Display for Error {
			#[inline]
			fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.write_str(match *self {
					$( Self::$k => $v, )+
				})
			}
		}
	);
}

err! {
	MissingSizeInfo       "Missing CD-Text size info.",
	InvalidPayloadLength  "Invalid CD-Text payload length.",
	UnsupportedExtension  "Unsupported CD-Text extension.",
	UnsupportedDoubleByte "Unsupported double-byte encoding.",
}



/// # Helper: Genre Code.
macro_rules! genre_code {
	( $( $k:ident $v:literal $str:literal, )+ ) => (
		#[repr(u8)]
		#[derive(Debug, Clone, Copy, Eq, PartialEq)]
		/// # CDText Genre Code.
		///
		/// Music that doesn't fit neatly into one of these predefined
		/// categories can optionally specify its genre as freeform text.
		enum GenreCode {
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

		impl fmt::Display for GenreCode {
			#[inline]
			fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.write_str(self.as_str())
			}
		}

		impl GenreCode {
			#[must_use]
			/// # From `u8`.
			const fn from_u8(raw: u8) -> Option<Self> {
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



/// # Helper: Language.
macro_rules! lang {
	( $( $k:ident $v:literal, )+ ) => (
		#[repr(u8)]
		#[derive(Debug, Clone, Copy, Eq, PartialEq)]
		/// # CDText Languages.
		///
		/// The language codes are specified in ANNEX 1..5 of EBU Tech 32 58 -E
		/// (1991).
		enum Language {
			$(
				#[doc = concat!("# ", stringify!($k), ".")]
				$k = $v,
			)+
		}

		/// # Sanity Checks.
		const _: () = {
			let mut all: &[Language] = &[$( Language::$k, )+];
			assert!(all[0] as u8 == 0, "BUG: First Language variant must be zero!");

			while let [ next, rest @ .. ] = all {
				// Languages count up from zero, but there are gaps.
				assert!(
					rest.is_empty() || (*next as u8) < (rest[0] as u8),
					"BUG: Languages are not sequential!",
				);
				// All codes should fit in the lower half of `u8`.
				assert!((*next as u8) < 128, "BUG: Language variants exceed `u7`.");
				all = rest;
			}
		};

		impl fmt::Display for Language {
			#[inline]
			fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.write_str(self.as_str())
			}
		}

		impl Language {
			#[must_use]
			/// # From `u8`.
			const fn from_u8(raw: u8) -> Option<Self> {
				match raw {
					$( $v => Some(Self::$k), )+
					_ => None,
				}
			}

			#[must_use]
			/// # As String Slice.
			const fn as_str(self) -> &'static str {
				match self {
					$( Self::$k => stringify!($k), )+
				}
			}
		}
	);
}

lang! {
	Unspecified   0x00,
	Albanian      0x01,
	Breton        0x02,
	Catalan       0x03,
	Croatian      0x04,
	Welsh         0x05,
	Czech         0x06,
	Danish        0x07,
	German        0x08,
	English       0x09,
	Spanish       0x0A,
	Esperanto     0x0B,
	Estonian      0x0C,
	Basque        0x0D,
	Faroese       0x0E,
	French        0x0F,
	Frisian       0x10,
	Irish         0x11,
	Gaelic        0x12,
	Galician      0x13,
	Icelandic     0x14,
	Italian       0x15,
	Lappish       0x16,
	Latin         0x17,
	Latvian       0x18,
	Luxembourgian 0x19,
	Lithuanian    0x1A,
	Hungarian     0x1B,
	Maltese       0x1C,
	Dutch         0x1D,
	Norwegian     0x1E,
	Occitan       0x1F,
	Polish        0x20,
	Portuguese    0x21,
	Romanian      0x22,
	Romansh       0x23,
	Serbian       0x24,
	Slovak        0x25,
	Slovenian     0x26,
	Finnish       0x27,
	Swedish       0x28,
	Turkish       0x29,
	Flemish       0x2A,
	Wallon        0x2B,
	// 0x2C..=0x44 are unassigned.
	Zulu          0x45,
	Vietnamese    0x46,
	Uzbek         0x47,
	Urdu          0x48,
	Ukrainian     0x49,
	Thai          0x4A,
	Telugu        0x4B,
	Tatar         0x4C,
	Tamil         0x4D,
	Tadzhik       0x4E,
	Swahili       0x4F,
	Sranantongo   0x50,
	Somali        0x51,
	Sinhalese     0x52,
	Shona         0x53,
	SerboCroat    0x54,
	// 0x55 is unassigned.
	Russian       0x56,
	Quechua       0x57,
	Pushtu        0x58,
	Punjabi       0x59,
	Persian       0x5A,
	Papamiento    0x5B,
	Oriya         0x5C,
	Nepali        0x5D,
	Ndebele       0x5E,
	Marathi       0x5F,
	Moldavian     0x60,
	Malaysian     0x61,
	Malagasay     0x62,
	Macedonian    0x63,
	Laotian       0x64,
	Korean        0x65,
	Khmer         0x66,
	Kazakh        0x67,
	Kannada       0x68,
	Japanese      0x69,
	Indonesian    0x6A,
	Hindi         0x6B,
	Hebrew        0x6C,
	Hausa         0x6D,
	Gurani        0x6E,
	Gujurati      0x6F,
	Greek         0x70,
	Georgian      0x71,
	Fulani        0x72,
	Dari          0x73,
	Churash       0x74,
	Chinese       0x75,
	Burmese       0x76,
	Bulgarian     0x77,
	Bengali       0x78,
	Bielorussian  0x79,
	Bambora       0x7A,
	Azerbaijani   0x7B,
	Assamese      0x7C,
	Armenian      0x7D,
	Arabic        0x7E,
	Amharic       0x7F,
}

impl Default for Language {
	#[inline]
	fn default() -> Self { Self::Unspecified }
}



/// # Helper: Pack Types.
macro_rules! pack {
	( $( $k:ident $v:literal $( $disc:ident $( $track:ident )? )?, )+ ) => (
		#[repr(u8)]
		#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
		/// # Pack Types.
		///
		/// This enum holds the low-level field codes used for actual querying.
		/// Most correspond to a logical disc and/or track field, but a few are
		/// reserved for informational data.
		enum PackKind {
			$(
				#[doc = concat!("# ", stringify!($k), ".")]
				$k = $v,
			)+
		}

		impl fmt::Display for PackKind {
			#[inline]
			fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				// Track label.
				if f.alternate() {
					self.track_field().map_or(
						Ok(()),
						|field| f.write_str(field.as_str()),
					)
				}
				// Disc label.
				else {
					self.disc_field().map_or(
						Ok(()),
						|field| f.write_str(field.as_str()),
					)
				}
			}
		}

		impl PackKind {
			#[must_use]
			/// # From `u8`.
			const fn from_u8(raw: u8) -> Option<Self> {
				match raw {
					$( $v => Some(Self::$k), )+
					_ => None,
				}
			}

			#[must_use]
			/// # Disc Field.
			const fn disc_field(self) -> Option<DiscField> {
				match self {
					$(
						$( Self::$k => Some(DiscField::$disc), )*
					)+
					_ => None,
				}
			}

			#[must_use]
			/// # Track Field.
			const fn track_field(self) -> Option<TrackField> {
				match self {
					$(
						$(
							$( Self::$k => Some(TrackField::$track), )*
						)*
					)+
					_ => None,
				}
			}

			#[must_use]
			/// # Internal Data?
			///
			/// Returns `true` if the field is not associatd with any text
			/// data.
			const fn is_data(self) -> bool {
				matches!(self, Self::TocInfo1 | Self::TocInfo2 | Self::SizeInfo)
			}
		}
	);
}

pack! {
	Title      0x80 Title      Title,
	Performer  0x81 Performer  Performer,
	Songwriter 0x82 Songwriter Songwriter,
	Composer   0x83 Composer   Composer,
	Arranger   0x84 Arranger   Arranger,
	Message    0x85 Message    Message,
	DiscId     0x86 DiscId,
	Genre      0x87 Genre,
	TocInfo1   0x88,
	TocInfo2   0x89,
	UpcEanIsrc 0x8E Barcode    Isrc,
	SizeInfo   0x8F,
}



#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
/// # (Block) Size Info.
///
/// This struct holds the sizing details for a block.
struct SizeInfo {
	/// # Encoding Code.
	char_code: u8,

	/// # First Track.
	first_track: u8,

	/// # Last Track.
	last_track: u8,

	/// # Copyright.
	///
	/// 3: CD-Text is copyrighted
	/// 0: no copyright on CD-Text
	copyright: u8,

	/// # Pack Counts.
	///
	/// 16 pack types (`0x80..=0x8F`).
	pack_counts: [u8; 16],

	/// # Last Sequence.
	///
	/// Sequence number for blocks `0..=7`.
	last_seq: [u8; 8],

	/// # Language Code.
	///
	/// Language code for blocks `0..=7`.
	lang_code: [u8; 8],
}

impl TryFrom<&[u8]> for SizeInfo {
	type Error = Error;

	fn try_from(buf: &[u8]) -> Result<Self, Self::Error> {
		if buf.len() != 36 {
			return Err(Error::InvalidPayloadLength);
		}

		let mut pack_counts = [0_u8; 16];
		pack_counts.copy_from_slice(&buf[4..20]);

		let mut last_seq = [0_u8; 8];
		last_seq.copy_from_slice(&buf[20..28]);

		let mut lang_code = [0_u8; 8];
		lang_code.copy_from_slice(&buf[28..36]);

		Ok(Self {
			char_code: buf[0],
			first_track: buf[1],
			last_track: buf[2],
			copyright: buf[3],
			pack_counts,
			last_seq,
			lang_code,
		})
	}
}

impl SizeInfo {
	#[must_use]
	/// # Total Expected Packs.
	fn total_expected_packs(&self) -> usize {
		self.pack_counts.iter().map(|&count| count as usize).sum()
	}
}



#[must_use]
/// # Validate Data Pack.
fn chk_pack(src: &Pack) -> bool {
	use crc::{Algorithm, Crc};

	// Define the exact CD-Text CRC-16 specification parameters.
	const CDTEXT_CRC: Algorithm<u16> = Algorithm {
		width: 16,
		poly: 0x1021,
		init: 0x0000,
		refin: false,
		refout: false,
		xorout: 0xFFFF,
		check: 0x2B8C,
		residue: 0x0000,
	};

	// The Engine.
	const ENGINE: Crc<u16> = Crc::<u16>::new(&CDTEXT_CRC);

	// Extract the expected CRC from the pack.
	let crc = u16::from_be_bytes([src[PACK_CRC_OFFSET], src[PACK_CRC_OFFSET + 1]]);

	// Compare with the actual checksum (of the rest).
	ENGINE.checksum(&src[..PACK_CRC_OFFSET]) == crc
}



#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn t_libcdio_samples() {
		macro_rules! compare {
			( $stub:literal ) => {
				let Some(parsed) = CDText::from_bytes(include_bytes!(
					concat!("../../skel/cdtext/", $stub, ".cdt")
				)) else {
					panic!("Failed to parse {}.cdt.", $stub);
				};
				let lhs = parsed.to_string();
				let rhs = include_str!(concat!("../../skel/cdtext/", $stub, ".right"));
				assert_eq!(
					lhs,
					rhs,
					"Mismatch for {}:\n\n-----\n{lhs}\n-----\n{rhs}\n",
					$stub,
				);
			};
		}

		compare!("0d10c613");
		compare!("640a6409");
		compare!("6708210a");
		compare!("7d050b0a");
		compare!("a308db0c");
		compare!("cdtext");
		compare!("cdtext-krosis");
		compare!("cdtext-libburnia");
		compare!("d60f430e");
		compare!("double");
		compare!("f310b110");
		compare!("simple");
	}
}
