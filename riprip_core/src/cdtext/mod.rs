/*!
# Rip Rip Hooray: CD-Text.

This module contains structures for parsing and storing CD-Text data.

Resources:
<https://libcdio.github.io/cd-text-format.html>
<https://github.com/xbmc/libcdio/blob/master/example/cdtext.c>
*/

mod parse;
mod field;
mod genre;
mod language;

use crate::{
	Barcode,
	macros::log,
};
use dactyl::NoHash;
pub use field::{
	DiscField,
	TrackField,
};
use genre::GenreCode;
use language::Language;
use parse::ContextBlock;
use std::{
	collections::HashMap,
	fmt,
};



#[derive(Debug, Default)]
/// # CD-Text!
///
/// This struct holds CD-Text in all available languages.
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
	/// # Decode Raw Pack Data.
	///
	/// Decode and return a structured representation of the CD-Text.
	///
	/// ## Errors
	///
	/// This method will return an error if the CD-Text is malformed,
	/// contains unsupported features, or is empty.
	pub(crate) fn from_bytes(pack_data: &[u8]) -> Result<Self, CDTextError> {
		// Build up the inner data.
		let blocks = ContextBlock::from_bytes(pack_data)?;
		let mut inner = Vec::with_capacity(blocks.len());
		for (i, block) in blocks.into_iter().enumerate().take(8) {
			// Parse the size info.
			let size_info = match block.size_info() {
				Ok(v) => v,
				Err(e) => {
					log!(@trace "{e}");
					return Err(e);
				},
			};

			let language = size_info.language(i)?;
			let encoding = size_info.encoding()?;
			let (first_track, last_track) = size_info.tracks();
			let mut catalog = HashMap::default();
			let mut genre_code = GenreCode::Unused;
			for ((field, track), buf) in block.into_buffer() {
				// Disc data?
				if track == 0 {
					if let Some(field) = field.disc_field() {
						let v = match field {
							// Don't accept invalid barcodes.
							DiscField::Barcode => {
								let Ok(v) = Barcode::try_from(buf.as_slice()) else {
									continue;
								};
								v.to_string()
							}
							// Separate genre code and content.
							DiscField::Genre => {
								let v = encoding.decode(&buf);
								let (v1, v2) = GenreCode::split_raw(v.as_bytes());
								genre_code = v1;
								if v2.is_empty() { continue; }
								v2.to_owned()
							},
							// Decode anything else.
							_ => { encoding.decode(&buf) },
						};
						if ! v.is_empty() {
							catalog.insert(
								u16::from_le_bytes([field as u8, 0]),
								v,
							);
						}
					}
				}
				// Track data?
				else if let Some(field) = field.track_field() {
					let v = encoding.decode(&buf);
					if ! v.is_empty() {
						catalog.insert(
							u16::from_le_bytes([field as u8, track]),
							v,
						);
					}
				}
			}

			// Save it!
			inner.push(CDTextInner {
				first_track,
				last_track,
				language,
				genre_code,
				catalog,
			});
		}

		// Done!
		if inner.is_empty() {
			std::hint::cold_path();
			Err(CDTextError::Empty)
		}
		else { Ok(Self(inner)) }
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
	ChecksumMismatch      "Unable to verify CD-Text data.",
	Empty                 "CD-Text exists, but is empty.",
	IncompleteData        "Raw CD-Text data is incomplete.",
	InvalidLanguage       "CD-Text contains invalid language code.",
	InvalidPayloadLength  "Invalid CD-Text payload length.",
	MissingSizeInfo       "Missing CD-Text size info.",
	UnsupportedDoubleByte "Unsupported CD-Text double-byte encoding.",
	UnsupportedEncoding   "Unsupported CD-Text encoding type.",
	UnsupportedExtension  "Unsupported CD-Text extension.",
}



#[derive(Debug, Default)]
/// # CD-Text (Inner).
///
/// This struct holds disc and track CD-Text in a single language.
struct CDTextInner {
	/// # First Track.
	first_track: u8,

	/// # Last Track.
	last_track: u8,

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
	const fn genre_code(&self) -> Option<GenreCode> {
		if self.genre_code.is_some() { Some(self.genre_code) }
		else { None }
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
			let Some(bin) = bins.remove(&stub) else {
				panic!("Missing {stub}.bin.");
			};

			let txt_v = std::fs::read_to_string(&txt).unwrap();
			let bin_v = std::fs::read(&bin).unwrap();
			let Ok(parsed) = CDText::from_bytes(&bin_v) else {
				panic!("Failed to parse {stub}.bin.");
			};
			let lhs = parsed.to_string();
			assert_eq!(
				lhs,
				txt_v,
				"Mismatch for {stub}:\n\n-----\nFOUND:\n{lhs}\n-----\nEXPECTED:\n{txt_v}\n",
			);
		}

		// There shouldn't be any bins left.
		assert!(
			bins.is_empty(),
			"Some CD-Text binary data is missing text counterparts: {bins:?}"
		);
	}
}
