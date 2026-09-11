/*!
# Rip Rip Hooray: CD-Text Parsing Context.
*/

use crate::macros::log;
use dactyl::traits::NiceInflection;
use std::{
	collections::HashMap,
	fmt,
};
use super::{
	CDTextError,
	DiscField,
	Encoding,
	Language,
	TrackField,
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
/// # Block Decoding Context.
struct Context {
	/// # Text Buffer.
	text_buf: Vec<u8>,

	/// # Language Blocks.
	language_blocks: Vec<ContextBlock>,
}

impl Context {
	/// # From Bytes.
	fn from_bytes(pack_data: &[u8]) -> Result<Self, CDTextError> {
		let mut out = Self::default();
		let (chunks, remainder) = pack_data.as_chunks::<PACK_LEN>();

		// A single trailing \0 byte is a tolerated convention, otherwise
		// assume parsing is incomplete.
		if ! remainder.is_empty() && remainder != [0] {
			log!(
				@trace
				"CD-Text parsing ended with {}.",
				remainder.len().nice_inflect("extra byte", "extra bytes"),
			);
			return Err(CDTextError::IncompleteData);
		}

		// Parse the chunks.
		for pack in chunks {
			if ! chk_pack(pack) {
				log!(@trace "Invalid CD-Text pack {pack:?}.");
				return Err(CDTextError::ChecksumMismatch);
			}
			if let Err(e) = out.parse_pack(pack) {
				log!(@trace "{e}");
				return Err(e);
			}
		}

		// Done!
		Ok(out)
	}

	/// # Parse Pack.
	fn parse_pack(&mut self, pack: &Pack) -> Result<(), CDTextError> {
		let header = &pack[0..PACK_HEADER_LEN];
		let payload = &pack[PACK_HEADER_LEN..PACK_CRC_OFFSET];

		let (id1, id2, _, id4) = (header[0], header[1], header[2], header[3]);

		let is_extension = (id2 & 0x80) != 0; // Extension Flag (0 = normal, 1 = extension)
		if is_extension {
			return Err(CDTextError::UnsupportedExtension);
		}
		let mut track_number = id2 & 0x7F;
		let block_id = (id4 >> 4) & 0x07; // Bits 4-6 define the language block ID.

		self.language_blocks
			.resize(block_id as usize + 1, ContextBlock::default());

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
				return Err(CDTextError::UnsupportedDoubleByte);
			}
			for b in payload {
				if *b == 0x00 {
					if ! self.text_buf.is_empty() {
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

	#[must_use]
	/// # Into Blocks.
	pub(super) fn into_blocks(self) -> Vec<ContextBlock> { self.language_blocks }
}



#[derive(Debug, Clone, Default)]
/// # Raw CD-Text Block.
pub(super) struct ContextBlock {
	/// # Pack Count.
	pack_count: usize,

	/// # Buffer.
	buffer: HashMap<(PackKind, u8), Vec<u8>>,
}

impl ContextBlock {
	/// # From Bytes.
	pub(super) fn from_bytes(pack_data: &[u8]) -> Result<Vec<Self>, CDTextError> {
		let context = Context::from_bytes(pack_data)?;
		let blocks = context.into_blocks();
		if blocks.is_empty() { Err(CDTextError::Empty) }
		else { Ok(blocks) }
	}

	/// # Size Info.
	pub(super) fn size_info(&self) -> Result<ContextSize, CDTextError> {
		let out = self.buffer.get(&(PackKind::SizeInfo, 0))
			.map(Vec::as_slice)
			.ok_or(CDTextError::MissingSizeInfo)
			.and_then(ContextSize::try_from)?;

		// Double check the counts match before returning.
		if self.pack_count == out.total_expected_packs() { Ok(out) }
		else { Err(CDTextError::IncompleteData) }
	}

	#[must_use]
	/// # Into Buffer.
	pub(super) fn into_buffer(self) -> HashMap<(PackKind, u8), Vec<u8>> {
		self.buffer
	}
}



#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
/// # (Block) Size Info.
///
/// This struct holds the sizing details for a block.
pub(super) struct ContextSize {
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

impl TryFrom<&[u8]> for ContextSize {
	type Error = CDTextError;

	fn try_from(buf: &[u8]) -> Result<Self, Self::Error> {
		if buf.len() != 36 {
			return Err(CDTextError::InvalidPayloadLength);
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

impl ContextSize {
	/// # Encoding.
	pub(super) fn encoding(&self) -> Result<Encoding, CDTextError> {
		let Some(encoding) = Encoding::from_u8(self.char_code) else {
			log!(@trace "Invalid/unsupported CD-Text encoding code: {}.", self.char_code);
			return Err(CDTextError::UnsupportedEncoding);
		};
		Ok(encoding)
	}

	/// # Language.
	pub(super) fn language(&self, idx: usize) -> Result<Language, CDTextError> {
		if self.lang_code.len() <= idx {
			std::hint::cold_path();
			log!(@trace "BUG: block index {idx:?} out of range!");
			return Err(CDTextError::InvalidLanguage);
		}

		let code = self.lang_code[idx];
		let Some(language) = Language::from_u8(code) else {
			log!(@trace "Invalid CD-Text language code: {code}.");
			return Err(CDTextError::InvalidLanguage);
		};
		Ok(language)
	}

	#[must_use]
	/// # Track Range.
	pub(super) const fn tracks(&self) -> (u8, u8) {
		(self.first_track, self.last_track)
	}

	#[must_use]
	/// # Total Expected Packs.
	fn total_expected_packs(&self) -> usize {
		self.pack_counts.iter().map(|&count| count as usize).sum()
	}
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
		pub(super) enum PackKind {
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
			pub(super) const fn disc_field(self) -> Option<DiscField> {
				match self {
					$(
						$( Self::$k => Some(DiscField::$disc), )*
					)+
					_ => None,
				}
			}

			#[must_use]
			/// # Track Field.
			pub(super) const fn track_field(self) -> Option<TrackField> {
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
