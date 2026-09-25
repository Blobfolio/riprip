/*!
# Rip Rip Hooray: CD-Text Stream Parsing.

This module deals with packs and blocks, reconstituting field data into whole
units.
*/

use crate::macros::log;
use dactyl::traits::NiceInflection;
use std::{
	collections::HashMap,
	fmt,
};
use super::{
	CDTextError,
	Encoding,
	DiscField,
	Language,
	TrackField,
	TrackRange,
};



/// # Pack Counts (by Type).
type PackCounts = [u8; PackKind::LEN];

/// # Pack Payload.
type PackPayload = [u8; Pack::PAYLOAD_LEN];

/// # Raw Pack.
type RawPack = [u8; Pack::RAW_LEN];



/// # Helper: Block IDs.
macro_rules! blockid {
	( $( $k:ident $v:literal, )+ ) => (
		#[repr(u8)]
		#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
		/// # Block ID.
		///
		/// CD-Text can store up to eight blocks of data. This enum is used
		/// to help reassure the compiler that e.g. array indexing can never
		/// fail.
		pub(super) enum BlockId {
			$(
				#[doc = concat!("# ", stringify!($k), ".")]
				$k = $v,
			)+
		}

		/// # Sanity Check.
		const _: () = {
			// Total is eight, starting at zero.
			assert!(
				(BlockId::MASK as usize) + 1 == BlockId::LEN,
				"BUG: `BlockId::MASK` is wrong.",
			);
			assert!(
				(BlockId::ALL[0] as u8) == 0,
				"BUG: `BlockId` must start at zero.",
			);

			// Check ordering and conversion.
			let mut all: &[BlockId] = BlockId::ALL.as_slice();
			while let [ next, rest @ .. ] = all {
				assert!(
					rest.is_empty() || (*next as u8) + 1 == (rest[0] as u8),
					"BUG: `BlockId`s are not sequential.",
				);
				assert!(
					(*next as u8) & BlockId::MASK == (*next as u8),
					"BUG: `BlockId` is out of range.",
				);
				assert!(
					(BlockId::from_u8(*next as u8) as u8) == (*next as u8),
					"BUG: `BlockId` <-> `u8` mismatches."
				);
				all = rest;
			}
		};

		impl BlockId {
			/// # All Block IDs.
			pub(super) const ALL: [Self; Self::LEN] = [ $( Self::$k, )+ ];

			/// # Length.
			pub(super) const LEN: usize = 8;

			/// # Mask.
			const MASK: u8 = 0b0000_0111;

			#[must_use]
			/// # From `u8`.
			///
			/// Note: only the lowest three bits are used for conversion.
			const fn from_u8(src: u8) -> Self {
				match src & Self::MASK {
					$( $v => Self::$k, )+
					_ => unreachable!(),
				}
			}
		}
	);
}

blockid! {
	Zero  0,
	One   1,
	Two   2,
	Three 3,
	Four  4,
	Five  5,
	Six   6,
	Seven 7,
}



#[derive(Debug, Clone, Default)]
/// # Raw CD-Text Block.
///
/// This struct holds the reconstituted (but still raw) field and metadata for
/// a single language block, along with a running count of the number of packs
/// contributing to it.
pub(super) struct Block {
	/// # Pack Count by Kind.
	pack_counts: PackCounts,

	/// # Field Buffers.
	buffers: HashMap<(PackKind, u8), Vec<u8>>,
}

impl Block {
	/// # From Pack Stream.
	///
	/// Reconstitute and return all block data from a complete stream of
	/// pack data.
	///
	/// Note that while eight blocks are assumed, in all likelihood only the
	/// first one or two will actually contain any data.
	///
	/// ## Errors
	///
	/// This method will bubble up any errors encountered, and return
	/// `CDTextError::Empty` if no blocks wind up being found.
	pub(super) fn from_stream(stream: &[u8])
	-> Result<[Self; BlockId::LEN], CDTextError> {
		let blocks = Blocks::from_stream(stream)?.into_blocks();
		if blocks.iter().any(Self::is_some) { Ok(blocks) }
		else { Err(CDTextError::Empty) }
	}
}

impl Block {
	#[must_use]
	/// # Is Some?
	///
	/// Returns `true` if any packs went into the making of the block.
	pub(super) const fn is_some(&self) -> bool {
		let mut i = 0;
		while i < self.pack_counts.len() {
			if self.pack_counts[i] != 0 { return true; }
			i += 1;
		}
		false
	}

	/// # Size Info.
	///
	/// This method pulls, parses, and returns the `BlockInfo` metadata for the
	/// block.
	///
	/// ## Errors
	///
	/// This will return an error if that data is missing or corrupt.
	pub(super) fn size_info(&self) -> Result<BlockInfo, CDTextError> {
		let out = self.buffers.get(&(PackKind::BlockInfo, 0))
			.map(Vec::as_slice)
			.ok_or(CDTextError::MissingBlockInfo)
			.and_then(BlockInfo::new)?;

		// Double check the counts match before returning.
		if self.pack_counts == out.pack_counts { Ok(out) }
		else { Err(CDTextError::IncompleteData) }
	}

	#[must_use]
	/// # Into Buffer.
	///
	/// Drop `self`, returning the type/track-specific buffers it had
	/// accumulated.
	pub(super) fn into_buffers(self) -> HashMap<(PackKind, u8), Vec<u8>> {
		self.buffers
	}
}



#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
/// # Block Info.
///
/// This struct holds the (relevant) block-level details encoded by packs of
/// type `PackKind::BlockInfo`.
///
/// (Every block should have exactly one of these.)
pub(super) struct BlockInfo {
	/// # Encoding Code.
	encoding_code: u8,

	/// # First and Last Tracks.
	tracks: TrackRange,

	/// # Pack Counts by Kind.
	///
	/// These totals are used to help verify that all expected packs were
	/// actually received.
	pack_counts: PackCounts,

	/// # Language Codes (All Blocks).
	///
	/// Language code for blocks `0..=7`.
	language_codes: [u8; BlockId::LEN],
}

impl BlockInfo {
	/// # From Bytes.
	///
	/// Convert the native 36-byte representation of this data into a new
	/// instance of `Self`, returning it.
	///
	/// ## Errors
	///
	/// This will return an error if the data has the wrong size, or the
	/// encoded track range is invalid.
	fn new(raw: &[u8]) -> Result<Self, CDTextError> {
		// This type of block has a fixed size.
		if raw.len() != 36 {
			return Err(CDTextError::InvalidBlockInfo);
		}

		// Encoding. (Defer actual mapping until later to improve error
		// reporting.)
		let encoding_code = raw[0];

		// Track range.
		let tracks = TrackRange::new(raw[1], raw[2])
			.ok_or(CDTextError::InvalidTrackRange)?;

		/*
		// Copyright: 0 (No), 3 (Yes).
		let copyright = raw[3];
		*/

		// Pack counts are encoded per type.
		let pack_counts = [
			raw[4],  raw[5],  raw[6],  raw[7],
			raw[8],  raw[9],  raw[10], raw[11],
			raw[12], raw[13], raw[14], raw[15],
			raw[16], raw[17], raw[18], raw[19],
		];

		/*
		// Sequences.
		let last_seq = [
			raw[20], raw[21], raw[22], raw[23],
			raw[24], raw[25], raw[26], raw[27],
		];
		*/

		// Language Codes (for all blocks).
		let language_codes = [
			raw[28], raw[29], raw[30], raw[31],
			raw[32], raw[33], raw[34], raw[35],
		];

		// Done!
		Ok(Self { encoding_code, tracks, pack_counts, language_codes })
	}

	/// # Encoding.
	///
	/// Return the encoding type, or an error if unsupported.
	pub(super) fn encoding(&self) -> Result<Encoding, CDTextError> {
		let Some(encoding) = Encoding::from_u8(self.encoding_code) else {
			log!(@trace "Invalid/unsupported CD-Text encoding code: 0x{:02X}.", self.encoding_code);
			return Err(CDTextError::UnsupportedEncoding);
		};
		Ok(encoding)
	}

	/// # Language.
	///
	/// Return the language associated with this block, or an error if the
	/// code is invalid.
	pub(super) fn language(&self, idx: BlockId) -> Result<Language, CDTextError> {
		let code = self.language_codes[idx as usize];
		let Some(language) = Language::from_u8(code) else {
			log!(@trace [idx, self.language_codes] "Invalid CD-Text language code: 0x{code:02X}.");
			return Err(CDTextError::InvalidLanguage);
		};
		Ok(language)
	}

	#[must_use]
	/// # Track Range.
	pub(super) const fn tracks(&self) -> TrackRange { self.tracks }
}



#[derive(Debug, Default)]
/// # Block Decoder.
///
/// Raw CD-Text is broken up into a stream of tiny-ass data packets. This
/// struct is used to parse and reconstitute those chunks into whole units,
/// grouped by block.
///
/// Note: this is used internally by `Block::from_stream`.
struct Blocks {
	/// # Value Buffer.
	buf: Vec<u8>,

	/// # Language Blocks.
	blocks: [Block; BlockId::LEN]
}

impl Blocks {
	/// # From Pack Stream.
	///
	/// Initialize and return `Self` from a complete stream of pack data.
	///
	/// ## Errors
	///
	/// This method will bubble up any errors encountered, and return
	/// `CDTextError::IncompleteData` if the stream cannot be broken up into
	/// individual packs.
	fn from_stream(packs: &[u8]) -> Result<Self, CDTextError> {
		let mut out = Self::default();
		let (chunks, rem) = packs.as_chunks::<{Pack::RAW_LEN}>();

		// The stream should divide evenly into pack-length packs, unless
		// trailed by a single null byte, which is a tolerated convention.
		if matches!(rem, [] | [0]) {
			// Build up the data pack-by-pack!
			for pack in chunks {
				if let Err(e) = Pack::new(pack).and_then(|v| out.push(&v)) {
					log!(@trace [pack] "{e}");
					return Err(e);
				}
			}

			// Done!
			Ok(out)
		}
		// Otherwise we should assume the stream is incomplete.
		else {
			log!(
				@trace
				"CD-Text parsing ended with {}.",
				rem.len().nice_inflect("extra byte", "extra bytes"),
			);
			Err(CDTextError::IncompleteData)
		}
	}

	/// # Push Pack.
	///
	/// Add the pack data to the appropriate block buffer.
	///
	/// ## Errors
	///
	/// This will return an error if the pack contains unsupported features.
	fn push(&mut self, pack: &Pack) -> Result<(), CDTextError> {
		// Extensions are unsupported.
		if pack.is_extension() { return Err(CDTextError::UnsupportedExtension); }

		// Pull track and block details.
		let mut track_number = pack.track_number();
		let block_id = pack.block_id();

		// What kind of pack is this? Note that per the CD-Text specification,
		// we can safely exit without an error if invalid.
		let Some(field) = pack.pack_kind() else { return Ok(()); };

		// Increase the pack count accordingly.
		self.blocks[block_id as usize].bump_count(field);

		// Internal data packs are simply chucked onto the associated block
		// buffer without any intermediary figuring.
		if field.is_data() {
			// Only save if the data is actually of use to us.
			if field.is_relevant_data() {
				let buffer = self.blocks[block_id as usize]
					.buffers
					.entry((field, 0))
					.or_default();
				buffer.extend_from_slice(pack.payload().as_slice());
			}
		}
		// Double-byte isn't supported.
		else if pack.is_double_byte() {
			return Err(CDTextError::UnsupportedDoubleByte);
		}
		// Otherwise file away the bytes, one at a time!
		else {
			for b in pack.payload() {
				match b {
					// Text is null-terminated. Copy the working buffer to the
					// block and reset for whatever comes next.
					b'\0' => {
						// Only copy if there's something to copy.
						if ! self.buf.is_empty() {
							self.blocks[block_id as usize]
								.buffers
								.insert((field, track_number), self.buf.clone());
							self.buf.clear();
						}

						// Bump the track number.
						track_number += 1;
					},

					// Tabs mark a repetition. Copy the previous block buffer
					// to the current slot.
					b'\t' => {
						if let Some(buf) = self.blocks[block_id as usize]
							.buffers
							.get(&(field, track_number.saturating_sub(1)))
							.cloned()
						{
							self.blocks[block_id as usize]
								.buffers
								.insert((field, track_number), buf);
						}
					},

					// Copy anything else to the working buffer.
					_ => { self.buf.push(b); },
				}
			}
		}

		// Done!
		Ok(())
	}

	#[must_use]
	/// # Into Blocks.
	///
	/// Drop `self`, returning the blocks it had accumulated.
	fn into_blocks(self) -> [Block; BlockId::LEN] { self.blocks }
}



#[derive(Debug, Clone, Copy)]
/// # Pack Data.
///
/// This struct holds the header and payload data from a raw 18-byte pack,
/// and exposes various methods for querying them.
///
/// (The last two bytes of the raw pack are discarded after being used to
/// validate the sixteen bytes we are keeping.)
struct Pack {
	/// # Header.
	header: [u8; Self::HEADER_LEN],

	/// # Payload.
	payload: PackPayload,
}

impl Pack {
	/// # Fixed Lookup Table.
	const CRC: [u16; 256] = [
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

	/// # Raw Pack Length.
	const RAW_LEN: usize = 18;

	/// # Header Length.
	const HEADER_LEN: usize = 4;

	/// # Payload Length.
	const PAYLOAD_LEN: usize = 12;

	/// # CRC Length.
	const CRC_LEN: usize = size_of::<u16>();

	/// # From Raw.
	///
	/// Packs comprise a 4-byte header, a 12-byte payload, and a 2-byte CRC
	/// checksum (big endian).
	///
	/// This method re-computes the data checksum and compares it against the
	/// stored value, returning the header and payload portions if valid.
	///
	/// ## Errors
	///
	/// This method will return an error if the pack data CRC does not match
	/// the stored value.
	const fn new(raw: &RawPack) -> Result<Self, CDTextError> {
		// Compute CRC of the header and payload data.
		let mut crc = 0_u16;
		let mut i = 0;
		while i < Self::HEADER_LEN + Self::PAYLOAD_LEN {
			let idx = (((crc >> 8) ^ (raw[i] as u16)) & 0xFF) as usize;
			crc = Self::CRC[idx] ^ (crc << 8);
			i += 1;
		}
		crc ^= 0xFFFF;

		// If the checksum matches the stored version, carve up that data and
		// return it!
		if crc == u16::from_be_bytes([raw[16], raw[17]]) {
			Ok(Self {
				header: [
					raw[0], // Pack Kind.
					raw[1], // Extension and Track Number.
					raw[2],
					raw[3], // Double-Byte and Block ID.
				],

				payload: [
					raw[4],  raw[5],  raw[6],  raw[7],
					raw[8],  raw[9],  raw[10], raw[11],
					raw[12], raw[13], raw[14], raw[15],
				],
			})
		}
		// Boo.
		else { Err(CDTextError::ChecksumMismatch) }
	}

	#[must_use]
	/// # Block ID.
	///
	/// Return the block ID, derived from bits `4..=6` of the fourth header
	/// byte. Note this value will always be in range `0..=7`.
	const fn block_id(&self) -> BlockId {
		BlockId::from_u8(self.header[3] >> 4)
	}

	#[must_use]
	/// # Is Double-Byte?
	///
	/// Returns `true` if the data is double-byte-encoded, using the highest
	/// bit of the fourth header byte.
	///
	/// Note: this is unsupported by Rip Rip.
	const fn is_double_byte(&self) -> bool { 0 != self.header[3] & 0b1000_0000 }

	#[must_use]
	/// # Is Extension?
	///
	/// Returns `true` if the pack is an extension, derived from the high bit
	/// of the second header byte.
	///
	/// Note: this is unsupported by Rip Rip.
	const fn is_extension(&self) -> bool { 0 != self.header[1] & 0b1000_0000 }

	#[must_use]
	/// # Pack Kind.
	///
	/// Return the `PackKind`, derived from the first header byte.
	const fn pack_kind(&self) -> Option<PackKind> { PackKind::from_u8(self.header[0]) }

	#[must_use]
	/// # Payload.
	///
	/// Return the data payload.
	const fn payload(&self) -> PackPayload { self.payload }

	#[must_use]
	/// # Track Number.
	///
	/// Return the track number, derived from the low 7 bits of the second
	/// header byte.
	const fn track_number(&self) -> u8 { self.header[1] & 0b0111_1111 }
}

/// # Sanity Check.
const _: () = {
	assert!(
		Pack::RAW_LEN == Pack::HEADER_LEN + Pack::PAYLOAD_LEN + Pack::CRC_LEN,
		"BUG: pack lengths are incorrect.",
	);
	assert!(
		16 == Pack::HEADER_LEN + Pack::PAYLOAD_LEN,
		"BUG: fixed CRC tables require exactly 16 bytes of data.",
	);
};



/// # Helper: Pack Types.
macro_rules! packkind {
	( $( $k:ident $v:literal $id:literal $( $disc:ident $( $track:ident )? )?, )+ ) => (
		#[repr(u8)]
		#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
		/// # Pack Types.
		///
		/// This enum represents the different types of packs.
		///
		/// While most types correspond directly to a disc and/or track field,
		/// a few are used to convey information about the block itself.
		pub(super) enum PackKind {
			$(
				#[doc = concat!("# ", stringify!($k), ".")]
				$k = $v,
			)+
		}

		/// # Sanity Checks.
		const _: () = {
			let mut all: &[PackKind] = PackKind::ALL.as_slice();
			while let [ next, rest @ .. ] = all {
				// Check sequence.
				assert!(
					rest.is_empty() || (*next as u8) + 1 == (rest[0] as u8),
					"BUG: `PackKind`s are not sequential.",
				);

				// Relevant data should be a subset of data.
				if next.is_relevant_data() {
					assert!(
						next.is_data(),
						"BUG: `PackKind::is_relevant_data` requires `is_data`.",
					);
				}
				all = rest;
			}

			// IDs should run 0..=15.
			let mut all: &[u8] = &[ $( $id, )+];
			assert!(all[0] == 0, "BUG: `PackKind` IDs must start at zero.");
			while let [ next, rest @ .. ] = all {
				assert!(
					rest.is_empty() || *next + 1 == rest[0],
					"BUG: `PackKind` IDs are not sequential.",
				);
				assert!(
					! rest.is_empty() || (*next as usize + 1) == PackKind::LEN,
					"BUG: last `PackKind` ID should be `PackKind::LEN - 1`.",
				);
				assert!(
					(PackKind::ALL[*next as usize] as u8) == *next + 0x80,
					"BUG: `PackKind` ID <=> VALUE mismatch.",
				);
				all = rest;
			}
		};

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
			/// # All Kinds.
			const ALL: [Self; Self::LEN] = [ $( Self::$k, )+ ];

			/// # Length.
			const LEN: usize = 16;

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
			/// Returns `true` if the type is not associated with a disc or
			/// track field.
			const fn is_data(self) -> bool {
				matches!(
					self,
					Self::TocInfo1 |
					Self::TocInfo2 |
					Self::Reserved1 |
					Self::Reserved2 |
					Self::Reserved3 |
					Self::ClosedInfo |
					Self::BlockInfo
				)
			}

			#[must_use]
			/// # Internal _and_ Relevant Data?
			///
			/// Returns `true` if the type `is_data` _and_ is something we
			/// actually need to store.
			const fn is_relevant_data(self) -> bool {
				matches!(self, Self::BlockInfo)
			}
		}

		// Add a counting helper here where the IDs are literals that won't
		// give the compiler any bounds-related concerns.
		impl Block {
			/// # Bump Count.
			///
			/// Increase the pack count for `kind` by one.
			const fn bump_count(&mut self, kind: PackKind) {
				let idx = match kind {
					$( PackKind::$k => $id, )+
				};
				self.pack_counts[idx] += 1;
			}
		}
	);
}

packkind! {
//  ----------------------------------------
//  PackKind   Val  ID DiscField  TrackField
//  ----------------------------------------
	Title      0x80  0 Title      Title,
	Performer  0x81  1 Performer  Performer,
	Songwriter 0x82  2 Songwriter Songwriter,
	Composer   0x83  3 Composer   Composer,
	Arranger   0x84  4 Arranger   Arranger,
	Message    0x85  5 Message    Message,
	DiscId     0x86  6 DiscId,
	Genre      0x87  7 Genre,
	TocInfo1   0x88  8,                      // Not used.
	TocInfo2   0x89  9,                      // Not used.
	Reserved1  0x8A 10,                      // Not used.
	Reserved2  0x8B 11,                      // Not used.
	Reserved3  0x8C 12,                      // Not used.
	ClosedInfo 0x8D 13,                      // Not used.
	UpcEanIsrc 0x8E 14 Barcode    Isrc,
	BlockInfo  0x8F 15,
}
