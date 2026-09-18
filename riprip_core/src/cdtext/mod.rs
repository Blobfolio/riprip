/*!
# Rip Rip Hooray: CD-Text.

This module contains structures for parsing and storing CD-Text data.

Resources:
<https://libcdio.github.io/cd-text-format.html>
<https://github.com/xbmc/libcdio/blob/master/example/cdtext.c>
*/

mod field;
mod genre;
mod language;
mod stream;
mod track;

use crate::{
	Barcode,
	Isrc,
	IsrcMap,
	macros::log,
};
use dactyl::NoHash;
pub use field::{
	DiscField,
	TrackField,
};
use genre::GenreCode;
use language::Language;
use std::{
	collections::HashMap,
	fmt,
};
use stream::{
	Block,
	BlockId,
};
use track::TrackRange;



#[derive(Debug, Default)]
/// # CD-Text!
///
/// This struct holds CD-Text in all available languages.
pub struct CDText {
	/// # Barcode.
	barcode: Option<Barcode>,

	/// # Track ISRCs.
	isrcs: IsrcMap,

	/// # Blocks.
	blocks: Vec<CDTextInner>,
}

impl fmt::Display for CDText {
	#[inline]
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let mut any = false;
		for (k, v) in self.blocks.iter().enumerate() {
			if any { writeln!(f)?; }
			else { any = true; }

			// Print the block header.
			if f.alternate() {
				if let Some(language) = v.language() {
					write!(f, "\n  [BLOCK {k}: {language}]")?;
				}
				else {
					write!(f, "\n  [BLOCK {k}]")?;
				}
			}
			else if let Some(language) = v.language() {
				writeln!(f, "[BLOCK {k}: {language}]")?;
			}
			else {
				writeln!(f, "[BLOCK {k}]")?;
			}

			// Defer for the rest.
			<CDTextInner as fmt::Display>::fmt(v, f)?;
		}
		Ok(())
	}
}

impl CDText {
	#[must_use]
	/// # Barcode.
	///
	/// Return the barcode defined by the first block, if any.
	pub(crate) const fn barcode(&self) -> Option<Barcode> { self.barcode }

	#[must_use]
	/// # ISRCs.
	///
	/// Return the track ISRCs defined by the first block, if any.
	pub(crate) fn isrcs(&self) -> Option<&IsrcMap> {
		if self.isrcs.is_empty() { None }
		else { Some(&self.isrcs) }
	}
}

impl CDText {
	/// # From Pack Stream.
	///
	/// Parse, decode, and return a structured representation of the CD-Text.
	///
	/// ## Errors
	///
	/// This method will return an error if the CD-Text is malformed,
	/// contains unsupported features, or is empty.
	pub(crate) fn from_bytes(pack_data: &[u8]) -> Result<Self, CDTextError> {
		// Build up the inner data block-by-block, skipping any unused
		// placeholders.
		let blocks = Block::from_stream(pack_data)?;
		let mut barcode = None;
		let mut isrcs = IsrcMap::with_hasher(NoHash::default());
		let mut inner = Vec::with_capacity(BlockId::LEN);
		for (i, block) in BlockId::ALL.into_iter().zip(blocks.into_iter().filter(Block::is_some)) {
			// Parse the size info.
			let size_info = match block.size_info() {
				Ok(v) => v,
				Err(e) => {
					log!(@trace [i] "{e}");
					return Err(e);
				},
			};

			let language = size_info.language(i)?;
			let encoding = size_info.encoding()?;
			let tracks = size_info.tracks();
			let mut catalog = HashMap::default();
			let mut genre_code = GenreCode::Unused;

			for ((field, track), buf) in block.into_buffers() {
				// Disc-level data.
				if track == 0 {
					if let Some(field) = field.disc_field() {
						let v = match field {
							// Force proper barcode formatting, skipping the
							// field if invalid.
							DiscField::Barcode => {
								let Ok(v) = Barcode::try_from(buf.as_slice()) else {
									continue;
								};

								// If the first block, copy the value to a
								// position of honor.
								if inner.is_empty() { barcode.replace(v); }

								v.to_string()
							}

							// Disc ID is always ASCII.
							DiscField::DiscId => { Encoding::Ascii.decode(&buf) },

							// Separate genre code and freeform representations.
							// Both are always ASCII.
							DiscField::Genre => {
								let (v1, v2) = GenreCode::split_raw(&buf);
								genre_code = v1;

								// Skip freeform insertion if empty.
								if v2.is_empty() { continue; }
								v2
							},

							// Everything else just needs to be decoded.
							_ => { encoding.decode(&buf) },
						};

						// Insert if non-empty!
						if ! v.is_empty() {
							catalog.insert(
								u16::from_le_bytes([field as u8, 0]),
								v,
							);
						}
					}
				}
				// Track-level data.
				else if let Some(field) = field.track_field() {
					let v = match field {
						// Force proper ISRC formatting.
						TrackField::Isrc => {
							let Ok(v) = Isrc::try_from(buf.as_slice()) else {
								continue;
							};

							// If the first block, copy the value to a
							// position of honor.
							if inner.is_empty() { isrcs.insert(track, v); }

							v.to_string()
						},

						// Everything else just needs to be decoded.
						_ => { encoding.decode(&buf) },
					};
					if ! v.is_empty() {
						catalog.insert(
							u16::from_le_bytes([field as u8, track]),
							v,
						);
					}
				}
			}

			// Save it!
			catalog.shrink_to_fit(); // This won't change.
			inner.push(CDTextInner { tracks, language, genre_code, catalog });
		}

		// Done!
		if inner.is_empty() {
			std::hint::cold_path();
			Err(CDTextError::Empty)
		}
		else {
			isrcs.shrink_to_fit(); // This won't change.
			Ok(Self {
				barcode,
				isrcs,
				blocks: inner,
			})
		}
	}
}



/// # Helper: Error.
macro_rules! err {
	( $( $k:ident $v:literal, )+ ) => (
		#[derive(Debug, Clone, Copy)]
		/// # CD-Text Decoding Errors.
		///
		/// This enum serves as a cheap error type for CD-Text decoding.
		pub(crate) enum CDTextError {
			$(
				#[doc = concat!("# ", $v)]
				$k,
			)+
		}

		impl fmt::Display for CDTextError {
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
	ChecksumMismatch      "CD-Text pack data failed checksum verification.",
	Empty                 "CD-Text exists, but is empty.",
	IncompleteData        "Raw CD-Text data is incomplete.",
	InvalidBlockInfo      "CD-Text block metadata is incomplete.",
	InvalidLanguage       "CD-Text contains invalid language code.",
	InvalidTrackRange     "Invalid start/end track range.",
	MissingBlockInfo      "Missing CD-Text block metadata.",
	UnsupportedDoubleByte "Unsupported CD-Text double-byte encoding.",
	UnsupportedEncoding   "Unsupported CD-Text encoding type.",
	UnsupportedExtension  "Unsupported CD-Text extension.",
}



#[derive(Debug, Default)]
/// # CD-Text (Inner).
///
/// This struct holds disc and track CD-Text in a single language.
struct CDTextInner {
	/// # First and Last Tracks.
	tracks: TrackRange,

	/// # Language.
	language: Language,

	/// # Genre Code.
	genre_code: GenreCode,

	/// # Data.
	catalog: HashMap<u16, String, NoHash>,
}

impl fmt::Display for CDTextInner {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		// Alternate formatting is used for the logger.
		if f.alternate() {
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
			for track in self.tracks.range() {
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
			for track in self.tracks.range() {
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
		let v = self.catalog.get(&u16::from_le_bytes([field as u8, 0]))?.trim();
		if v.is_empty() { None }
		else { Some(v) }
	}

	#[must_use]
	/// # Language.
	const fn language(&self) -> Option<Language> {
		if self.language.is_some() { Some(self.language) }
		else { None }
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
	const fn genre_code(&self) -> Option<GenreCode> {
		if self.genre_code.is_some() { Some(self.genre_code) }
		else { None }
	}
}



#[repr(u8)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
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
	/// Parse a raw byte stream into a string, given the encoding type.
	///
	/// This method trims all results, normalizes inner whitespace to
	/// horizontal spaces, and drops control characters.
	fn decode(self, bytes: &[u8]) -> String {
		use trimothy::TrimMut;

		/// # Normalize Char.
		///
		/// Convert whitespace to a horizontal space, drop controls.
		const fn normalize_char(c: char) -> Option<char> {
			if c.is_whitespace() { Some(' ' ) }
			else if c.is_control() { None }
			else { Some(c) }
		}

		// Skip the trouble.
		if bytes.is_empty() { return String::new(); }

		// Convert to string, with normalized whitespace and no controls.
		let mut out: String = match self {
			Self::Ascii =>
				if bytes.is_ascii() {
					bytes.iter()
						.copied()
						.filter_map(|b| normalize_char(b as char))
						.collect()
				}
				// LIAR!
				else { return String::new(); },

			// Mapping works exactly the same as ASCII, minus the ASCII
			// requirement.
			Self::Iso8859_1 => bytes.iter()
				.copied()
				.filter_map(|b| normalize_char(b as char))
				.collect(),

			// This one requires specialized UTF-8 conversion prior to
			// normalization.
			Self::ShiftJis => encoding_rs::SHIFT_JIS.decode(bytes)
				.0
				.chars()
				.filter_map(normalize_char)
				.collect(),
		};

		// Trim the edges and return.
		out.trim_mut();
		out
	}
}



#[cfg(test)]
mod test {
	use super::*;
	use std::{
		ffi::OsStr,
		path::PathBuf,
	};

	#[test]
	fn t_cdtext() {
		let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("skel/cdtext");
		if ! dir.is_dir() {
			panic!("Invalid cdtext test directory.");
		}

		// Find all the files. Haha.
		let mut bins = HashMap::<String, PathBuf>::new();
		let mut txts = HashMap::<String, PathBuf>::new();
		for e in std::fs::read_dir(&dir).expect("Unable to open cdtext test directory.") {
			let e = e.unwrap();
			let path = e.path();
			let stub = path.file_stem().and_then(OsStr::to_str).unwrap();
			let ext = path.extension().and_then(OsStr::to_str).unwrap();
			match ext {
				"bin" => { bins.insert(stub.to_owned(), path); },
				"txt" => { txts.insert(stub.to_owned(), path); },
				_ => {
					panic!(
						"Unexpected file in cdtext test directory: {}",
						path.file_name().unwrap().display()
					);
				},
			}
		}

		// Run through and compare!
		assert!(
			! txts.is_empty(),
			"No CD-Text tests were found.",
		);
		for (stub, txt) in txts {
			// Pull the matching bin.
			let Some(bin) = bins.remove(&stub) else {
				panic!("Missing {stub}.bin.");
			};

			// Read both.
			let txt_v = std::fs::read_to_string(&txt).unwrap();
			let mut bin_v = std::fs::read(&bin).unwrap();

			// None of the tests are empty.
			assert!(! bin_v.is_empty(), "{stub}.bin is empty.");

			// Parse the binary version and compare its text with the expected
			// text.
			let parsed = match CDText::from_bytes(&bin_v) {
				Ok(v) => v.to_string(),
				Err(e) => panic!("Failed to parse {stub}.bin: {e}"),
			};
			assert_eq!(
				parsed,
				txt_v,
				"Mismatch for {stub}:\n\n-----\nFOUND:\n{parsed}\n-----\nEXPECTED:\n{txt_v}\n",
			);

			// Ensure (simple) corruption is properly detected during parsing.
			bin_v[0] = bin_v[0].wrapping_add(1);
			assert!(
				matches!(CDText::from_bytes(&bin_v), Err(CDTextError::ChecksumMismatch)),
				"Corrupted {stub}.bin failed to trigger checksum mismatch.",
			);
		}

		// There shouldn't be any bins left.
		assert!(
			bins.is_empty(),
			"Some CD-Text binary data is missing text counterparts: {bins:?}"
		);
	}

	#[test]
	fn t_cdtext_empty() {
		assert!(
			matches!(CDText::from_bytes(&[]), Err(CDTextError::Empty)),
			"Empty CD-Text parsed Ok().",
		);
	}

	#[test]
	fn t_decode_normalize() {
		assert_eq!(
			Encoding::Ascii.decode(b"hello world"),
			"hello world",
		);
		assert_eq!(
			Encoding::Ascii.decode(b" \n\0hello\tworld \0\0 "),
			"hello world",
		);
		assert_eq!(
			Encoding::Ascii.decode("hello ♥".as_bytes()), // Not ASCII!
			"",
		);

		// Latin probably shouldn't be for lovers.
		let raw: &[u8] = &[
			b'h', 233, b'l', b'l', 246,  b' ',
			b'w', 246, b'r', b'l', b'd', b'!',
		];
		let dec = Encoding::Iso8859_1.decode(raw);
		let exp = "héllö wörld!";

		assert_eq!(dec, exp);                       // Looks right.
		assert_eq!(raw.len(), dec.chars().count()); // Same char count.
		assert!(raw.len() < dec.len());             // But different/more bytes!
	}
}
