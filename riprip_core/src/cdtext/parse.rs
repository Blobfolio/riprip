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

/// # Pack Data Length.
const PACK_DATA_LEN: usize = PACK_HEADER_LEN + PACK_PAYLOAD_LEN;

/// # Data Pack.
type Pack = [u8; PACK_LEN];

/// # Pack Payload.
type PackPayload = [u8; PACK_PAYLOAD_LEN];

/// # Sanity Check.
const _: () = {
	assert!(
		PACK_DATA_LEN == 16 &&
		PACK_LEN == PACK_DATA_LEN + size_of::<u16>(),
		"BUG: PACK_LEN must be 16 bytes of data plus a two-byte CRC.",
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
		// The size block is exactly 36 bytes.
		if buf.len() != 36 {
			return Err(CDTextError::InvalidPayloadLength);
		}

		// Small stuff.
		let char_code = buf[0];
		let tracks = TrackRange::new(buf[1], buf[2])?;
		let copyright = buf[3];

		// Pack counts.
		let pack_counts = [
			buf[4],  buf[5],  buf[6],  buf[7],
			buf[8],  buf[9],  buf[10], buf[11],
			buf[12], buf[13], buf[14], buf[15],
			buf[16], buf[17], buf[18], buf[19],
		];

		// Sequences.
		let last_seq = [
			buf[20], buf[21], buf[22], buf[23],
			buf[24], buf[25], buf[26], buf[27],
		];

		// Language Codes.
		let lang_code = [
			buf[28], buf[29], buf[30], buf[31],
			buf[32], buf[33], buf[34], buf[35],
		];

		Ok(Self {
			char_code,
			tracks,
			copyright,
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
///
/// Packs comprise 16 bytes of data and a 2-byte CRC value (big endian). This
/// method (re)checks the data, returning `true` if the computed checksum
/// matches the stored/expected value.
const fn chk_pack(src: &Pack) -> bool {
	/// # Fixed Lookup Table.
	const TABLE: [u16; 256] = [
		0x0000, 0x1021, 0x2042, 0x3063, 0x4084, 0x50A5, 0x60C6, 0x70E7, 0x8108, 0x9129, 0xA14A, 0xB16B, 0xC18C, 0xD1AD, 0xE1CE, 0xF1EF,
		0x1231, 0x0210, 0x3273, 0x2252, 0x52B5, 0x4294, 0x72F7, 0x62D6, 0x9339, 0x8318, 0xB37B, 0xA35A, 0xD3BD, 0xC39C, 0xF3FF, 0xE3DE,
		0x2462, 0x3443, 0x0420, 0x1401, 0x64E6, 0x74C7, 0x44A4, 0x5485, 0xA56A, 0xB54B, 0x8528, 0x9509, 0xE5EE, 0xF5CF, 0xC5AC, 0xD58D,
		0x3653, 0x2672, 0x1611, 0x0630, 0x76D7, 0x66F6, 0x5695, 0x46B4, 0xB75B, 0xA77A, 0x9719, 0x8738, 0xF7DF, 0xE7FE, 0xD79D, 0xC7BC,
		0x48C4, 0x58E5, 0x6886, 0x78A7, 0x0840, 0x1861, 0x2802, 0x3823, 0xC9CC, 0xD9ED, 0xE98E, 0xF9AF, 0x8948, 0x9969, 0xA90A, 0xB92B,
		0x5AF5, 0x4AD4, 0x7AB7, 0x6A96, 0x1A71, 0x0A50, 0x3A33, 0x2A12, 0xDBFD, 0xCBDC, 0xFBBF, 0xEB9E, 0x9B79, 0x8B58, 0xBB3B, 0xAB1A,
		0x6CA6, 0x7C87, 0x4CE4, 0x5CC5, 0x2C22, 0x3C03, 0x0C60, 0x1C41, 0xEDAE, 0xFD8F, 0xCDEC, 0xDDCD, 0xAD2A, 0xBD0B, 0x8D68, 0x9D49,
		0x7E97, 0x6EB6, 0x5ED5, 0x4EF4, 0x3E13, 0x2E32, 0x1E51, 0x0E70, 0xFF9F, 0xEFBE, 0xDFDD, 0xCFFC, 0xBF1B, 0xAF3A, 0x9F59, 0x8F78,
		0x9188, 0x81A9, 0xB1CA, 0xA1EB, 0xD10C, 0xC12D, 0xF14E, 0xE16F, 0x1080, 0x00A1, 0x30C2, 0x20E3, 0x5004, 0x4025, 0x7046, 0x6067,
		0x83B9, 0x9398, 0xA3FB, 0xB3DA, 0xC33D, 0xD31C, 0xE37F, 0xF35E, 0x02B1, 0x1290, 0x22F3, 0x32D2, 0x4235, 0x5214, 0x6277, 0x7256,
		0xB5EA, 0xA5CB, 0x95A8, 0x8589, 0xF56E, 0xE54F, 0xD52C, 0xC50D, 0x34E2, 0x24C3, 0x14A0, 0x0481, 0x7466, 0x6447, 0x5424, 0x4405,
		0xA7DB, 0xB7FA, 0x8799, 0x97B8, 0xE75F, 0xF77E, 0xC71D, 0xD73C, 0x26D3, 0x36F2, 0x0691, 0x16B0, 0x6657, 0x7676, 0x4615, 0x5634,
		0xD94C, 0xC96D, 0xF90E, 0xE92F, 0x99C8, 0x89E9, 0xB98A, 0xA9AB, 0x5844, 0x4865, 0x7806, 0x6827, 0x18C0, 0x08E1, 0x3882, 0x28A3,
		0xCB7D, 0xDB5C, 0xEB3F, 0xFB1E, 0x8BF9, 0x9BD8, 0xABBB, 0xBB9A, 0x4A75, 0x5A54, 0x6A37, 0x7A16, 0x0AF1, 0x1AD0, 0x2AB3, 0x3A92,
		0xFD2E, 0xED0F, 0xDD6C, 0xCD4D, 0xBDAA, 0xAD8B, 0x9DE8, 0x8DC9, 0x7C26, 0x6C07, 0x5C64, 0x4C45, 0x3CA2, 0x2C83, 0x1CE0, 0x0CC1,
		0xEF1F, 0xFF3E, 0xCF5D, 0xDF7C, 0xAF9B, 0xBFBA, 0x8FD9, 0x9FF8, 0x6E17, 0x7E36, 0x4E55, 0x5E74, 0x2E93, 0x3EB2, 0x0ED1, 0x1EF0,
	];

	// Calculate.
	let mut crc = 0_u16;
	let mut i = 0;
	while i < PACK_DATA_LEN {
		let idx = (((crc >> 8) ^ (src[i] as u16)) & 0xFF) as usize;
		crc = TABLE[idx] ^ (crc << 8);
		i += 1;
	}
	crc ^= 0xFFFF;

	// Compare!
	crc == u16::from_be_bytes([src[PACK_DATA_LEN], src[PACK_DATA_LEN + 1]])
}
