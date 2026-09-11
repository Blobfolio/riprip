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
	TrackRange,
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

/// # Pack Payload.
type PackPayload = [u8; PACK_PAYLOAD_LEN];

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
	fn from_bytes(packs: &[u8]) -> Result<Self, CDTextError> {
		let mut out = Self::default();
		let (chunks, remainder) = packs.as_chunks::<PACK_LEN>();

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

		// Parse the packs.
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
		let (header, payload) = PackHeader::split(pack);

		// Extensions are unsupported.
		if header.is_extension() { return Err(CDTextError::UnsupportedExtension); }

		// Pull track and block details.
		let mut track_number = header.track_number();
		let block_id = header.block_id();

		// Make sure we have a corresponding entry.
		self.language_blocks.resize(
			block_id as usize + 1,
			ContextBlock::default(),
		);

		// Increase the pack count accordingly.
		self.language_blocks[block_id as usize].pack_count += 1;

		// What kind of pack is this? Note that per the CD-Text specification,
		// we can safely exit without an error if invalid.
		let Some(field) = header.pack_kind() else { return Ok(()); };

		// If data (not text), just create/append the payload to the matching
		// block buffer.
		if field.is_data() {
			let key = (field, 0);
			let buffer = self.language_blocks[block_id as usize]
				.buffer
				.entry(key)
				.or_default();
			buffer.extend_from_slice(payload.as_slice());
		}
		// Double-byte isn't supported.
		else if header.is_double_byte() {
			return Err(CDTextError::UnsupportedDoubleByte);
		}
		// Otherwise file away the bytes, one at a time!
		else {
			for b in payload {
				match b {
					// Text is null-terminated. Save the current buffer and
					// reset.
					b'\0' => {
						// Assuming we've got a buffer, that is.
						if ! self.text_buf.is_empty() {
							let key = (field, track_number);
							self.language_blocks[block_id as usize]
								.buffer
								.insert(key, self.text_buf.clone());
							self.text_buf.clear();
						}

						// Bump the track number.
						track_number += 1;
					},

					// Tabs mark a repetition. Copy the previous block buffer
					// to the current slot.
					b'\t' => {
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
					},

					// Anything else is a work-in-progress. Push the byte to
					// the text buffer.
					_ => { self.text_buf.push(b); },
				}
			}
		}

		// Done!
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

	/// # First and Last Tracks.
	tracks: TrackRange,

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

		let tracks = TrackRange::new(buf[1], buf[2])?;

		let mut pack_counts = [0_u8; 16];
		pack_counts.copy_from_slice(&buf[4..20]);

		let mut last_seq = [0_u8; 8];
		last_seq.copy_from_slice(&buf[20..28]);

		let mut lang_code = [0_u8; 8];
		lang_code.copy_from_slice(&buf[28..36]);

		Ok(Self {
			char_code: buf[0],
			tracks,
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
	pub(super) const fn tracks(&self) -> TrackRange { self.tracks }

	#[must_use]
	/// # Total Expected Packs.
	fn total_expected_packs(&self) -> usize {
		self.pack_counts.iter().map(|&count| count as usize).sum()
	}
}



#[derive(Debug, Clone, Copy)]
/// # Pack Header.
struct PackHeader([u8; PACK_HEADER_LEN]);

impl PackHeader {
	#[must_use]
	/// # Split Header/Payload.
	///
	/// Split the header from the pack data, returning it and the rest of the
	/// data (i.e. the payload).
	const fn split(pack: &Pack) -> (Self, PackPayload) {
		let header = Self([
			pack[0], // Pack Kind.
			pack[1], // Extension and Track Number.
			pack[2],
			pack[3], // Double-Byte and Block ID.
		]);

		let payload = [
			pack[4],  pack[5],  pack[6],  pack[7],
			pack[8],  pack[9],  pack[10], pack[11],
			pack[12], pack[13], pack[14], pack[15],
		];

		(header, payload)
	}

	#[must_use]
	/// # Block ID.
	///
	/// Return the block ID, derived from bits `4..=6` of the fourth header
	/// byte. Note this value will always be in range `0..=7`.
	const fn block_id(self) -> u8 { (self.0[3] >> 4) & 0b0000_0111 }

	#[must_use]
	/// # Is Double-Byte?
	///
	/// Returns `true` if the data is double-byte-encoded, using the highest
	/// bit of the fourth header byte.
	///
	/// Note: this is unsupported by Rip Rip.
	const fn is_double_byte(self) -> bool { 0 != self.0[3] & 0b1000_0000 }

	#[must_use]
	/// # Is Extension?
	///
	/// Returns `true` if the pack is an extension, derived from the high bit
	/// of the second header byte.
	///
	/// Note: this is unsupported by Rip Rip.
	const fn is_extension(self) -> bool { 0 != self.0[1] & 0b1000_0000 }

	#[must_use]
	/// # Pack Kind.
	///
	/// Return the `PackKind`, derived from the first header byte.
	const fn pack_kind(self) -> Option<PackKind> { PackKind::from_u8(self.0[0]) }

	#[must_use]
	/// # Track Number.
	///
	/// Return the track number, derived from the low 7 bits of the second
	/// header byte.
	const fn track_number(self) -> u8 { self.0[1] & 0b0111_1111 }
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
