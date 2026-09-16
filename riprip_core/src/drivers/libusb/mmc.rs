/*!
# Rip Rip Hooray: SCSI Multimedia Commands

Provides opcodes, profiles, and format constants for SCSI MMC and core SPC commands
used to inspect and read audio CDs.
*/

use crate::{
	Barcode, CD_LEADIN, CD_LEADOUT, CddaDriverExt, DriveVendorModel, RipRipError, macros::log,
};

// Opcodes
const READ_SUB_CHANNEL: u8 = 0x42;
const READ_TOC: u8 = 0x43;
const GET_CONFIGURATION: u8 = 0x46;
const READ_CD: u8 = 0xBE;

// Architectural Constraints
const FIRST_TRACK: u8 = 0x01;
const MAX_TRACK_NUMBER: u8 = 99;

// READ_SUB_CHANNEL Data Formats
const SUB_FORMAT_MCN: u8 = 0x02;

const SUB_CHANNEL_HEADER_LEN: usize = 4;
const SUB_CHANNEL_MCN_DATA_LEN: usize = 22;

// READ_TOC Time/Address Format
const FORMAT_LBA: u8 = 0x00;
const FORMAT_MSF: u8 = 0x02;

// READ_TOC Format Codes
const TOC_FORMAT_TOC: u8 = 0x00;
const TOC_FORMAT_CDTEXT: u8 = 0x05;

pub(super) const TOC_HEADER_LEN: usize = 4;
const TOC_TRACK_DESCRIPTOR_LEN: usize = 8;

pub(super) const CTRL_DATA_TRACK: u8 = 0x04; // Bitmask for track type: set = Data, cleared = Audio.

// GET_CONFIGURATION Features
const FEATURE_CD_AUDIO_C2: u16 = 0x001E;

const CONFIGURATION_HEADER_LEN: usize = 8;
const CONFIGURATION_FEATURE_DESCRIPTOR_LEN: usize = 8;

// READ_CD Sector Types
const SECTOR_TYPE_CDDA: u8 = 0x04;

/// `spc` (SCSI Primary Commands) covers baseline commands that every SCSI device must understand,
/// regardless of what it is (like INQUIRY or TEST_UNIT_READY).
mod spc {
	pub(super) const INQUIRY: u8 = 0x12;

	pub(super) const INQUIRY_HEADER_LEN: usize = 8;
	pub(super) const INQUIRY_VENDOR_ID_LEN: usize = 8;
	pub(super) const INQUIRY_PRODUCT_ID_LEN: usize = 16;
	pub(super) const INQUIRY_REVISION_LEVEL_LEN: usize = 4;
}

const fn to_be_u8(value: usize) -> [u8; 1] {
	assert!(value <= u8::MAX as usize, "value does not fit in u8");

	let bytes = value.to_be_bytes();
	[bytes[bytes.len() - 1]]
}

const fn to_be_u16(value: usize) -> [u8; 2] {
	assert!(value <= u16::MAX as usize, "value does not fit in u16");

	let bytes = value.to_be_bytes();
	[bytes[bytes.len() - 2], bytes[bytes.len() - 1]]
}

const fn copy_bytes<const N: usize, const M: usize>(
	dst: &mut [u8; N],
	range: std::ops::Range<usize>,
	src: [u8; M],
) {
	let start = range.start;
	let end = range.end;

	assert!(start <= end, "invalid byte range");
	assert!(end <= N, "range exceeds destination");
	assert!(end - start == M, "byte range does not match value size");

	let mut i = 0;
	while i < M {
		dst[start + i] = src[i];
		i += 1;
	}
}

pub(super) trait TransportExt {
	/// Sends a SCSI Command Descriptor Block (CDB) and transfers data from the device.
	fn submit<const N: usize>(&self, cdb: &[u8; N], data: &mut [u8]) -> Result<usize, RipRipError>;
}

/// A low-level API for interacting with a physical CD drive specifically to read audio data.
///
/// Relies on the underlying `TransportExt` trait to handle the hardware bus communication
/// (e.g. USB BOT or `/dev/sg`).
pub(super) trait MmcDriverExt: TransportExt {
	fn mcn_subchannel__(&self) -> Result<Option<Barcode>, RipRipError> {
		const ALLOC_LEN: usize = SUB_CHANNEL_HEADER_LEN + SUB_CHANNEL_MCN_DATA_LEN;

		const CDB: [u8; 10] = {
			let mut cdb = [0; _];
			cdb[0] = READ_SUB_CHANNEL;
			cdb[1] = FORMAT_MSF;
			cdb[2] = 0x40; // Sub-Q Channel tracking bit
			cdb[3] = SUB_FORMAT_MCN;

			copy_bytes(&mut cdb, 7..9, to_be_u16(ALLOC_LEN));
			cdb
		};

		let mut buf = [0u8; ALLOC_LEN];
		if self.submit(&CDB, &mut buf)? < ALLOC_LEN {
			log!(@trace "Subchannel contains no MCN data.");
			return Ok(None);
		}

		let data_format = buf[3];
		let subq_element_valid = buf[4];

		if data_format == SUB_FORMAT_MCN && subq_element_valid == 0x01 {
			// Bit 7 tracks string validation rules (MCVAL flag in MMC spec).
			let is_mcn_valid = (buf[12] & 0x80) != 0;
			if is_mcn_valid {
				let raw_ascii = &buf[13..26];
				return Barcode::try_from(raw_ascii).map(Some);
			}
		}

		log!(@trace "Subchannel contains no MCN data.");
		Ok(None)
	}

	fn check_disc_mode__(&self) -> Result<(), RipRipError> {
		const ALLOC_LEN: usize = TOC_HEADER_LEN;

		const CDB: [u8; 10] = {
			let mut cdb = [0; _];
			cdb[0] = READ_TOC;
			cdb[1] = FORMAT_LBA;
			cdb[2] = TOC_FORMAT_TOC; // Format 0: Standard Table of Contents.
			cdb[6] = FIRST_TRACK; // Starting track.

			copy_bytes(&mut cdb, 7..9, to_be_u16(ALLOC_LEN));
			cdb
		};

		// Asks only for enough bytes to discover how large the TOC is.
		let mut buf = [0u8; ALLOC_LEN];
		if self.submit(&CDB, &mut buf)? < TOC_HEADER_LEN {
			return Err(RipRipError::DiscMode);
		}

		let toc_len = u16::from_be_bytes([buf[0], buf[1]])
			.checked_add(2) // Length excludes the 2-byte length field itself.
			.ok_or(RipRipError::DiscMode)?;

		// Patch the CDB with the expected allocation length.
		let mut cdb = CDB;
		copy_bytes(&mut cdb, 7..9, toc_len.to_be_bytes());

		let mut buf = vec![0u8; toc_len.into()];
		let len = self.submit(&cdb, &mut buf)?;
		if len < TOC_HEADER_LEN {
			return Err(RipRipError::DiscMode);
		}

		let first_track = buf[2];
		let last_track = buf[3];

		// Sanity check.
		if last_track == 0 || first_track > last_track || last_track > MAX_TRACK_NUMBER {
			return Err(RipRipError::DiscMode);
		}

		let track_count = (last_track - first_track + 1) as usize;

		let total_count = track_count + 1; // Lead-out.

		let required_len = TOC_HEADER_LEN + total_count * TOC_TRACK_DESCRIPTOR_LEN;
		if len < required_len {
			return Err(RipRipError::DiscMode);
		}

		// Search the descriptors. If an audio track is found, early exit,
		// otherwise default to a DiscMode error.
		let has_audio = buf[TOC_HEADER_LEN..]
			.as_chunks::<TOC_TRACK_DESCRIPTOR_LEN>()
			.0
			.iter()
			.take(track_count)
			.any(|desc| (desc[1] & CTRL_DATA_TRACK) == 0);

		if has_audio {
			Ok(())
		} else {
			Err(RipRipError::DiscMode)
		}
	}

	fn check_c2__(&self) -> Result<(), RipRipError> {
		const ALLOC_LEN: usize = CONFIGURATION_HEADER_LEN + CONFIGURATION_FEATURE_DESCRIPTOR_LEN;

		const CDB: [u8; 10] = {
			let mut cdb = [0; _];
			cdb[0] = GET_CONFIGURATION;
			cdb[1] = 0x02; // RT field = 0x02: Request only the specific feature specified in bytes 2-3.
			copy_bytes(&mut cdb, 2..4, FEATURE_CD_AUDIO_C2.to_be_bytes());

			copy_bytes(&mut cdb, 7..9, to_be_u16(ALLOC_LEN));
			cdb
		};

		let mut buf = [0u8; ALLOC_LEN];
		if self.submit(&CDB, &mut buf)? < ALLOC_LEN {
			return Err(RipRipError::C2Mode296);
		}

		let desc = &buf[CONFIGURATION_HEADER_LEN..];

		if u16::from_be_bytes([desc[0], desc[1]]) == FEATURE_CD_AUDIO_C2 {
			// Byte 4 houses the Feature-Specific configuration flags.
			// Bit 0 is the C2 Validity flag (indicates drive can deliver C2 data over bus pipelines).
			if (desc[4] & 0x01) != 0 {
				return Ok(());
			}
		}

		Err(RipRipError::C2Mode296)
	}

	fn read_cdtext(&self) -> Result<Option<Vec<u8>>, RipRipError> {
		const ALLOC_LEN: usize = TOC_HEADER_LEN;

		const CDB: [u8; 10] = {
			let mut cdb = [0; _];
			cdb[0] = READ_TOC;
			cdb[1] = FORMAT_LBA;
			cdb[2] = TOC_FORMAT_CDTEXT;
			cdb[6] = 0; // Track number to start reading from (0 = entire disc).

			copy_bytes(&mut cdb, 7..9, to_be_u16(ALLOC_LEN));
			cdb
		};

		// Asks only for enough bytes to discover how large the CD-Text is.
		let mut buf = [0u8; ALLOC_LEN];
		if self.submit(&CDB, &mut buf)? < ALLOC_LEN {
			return Err(RipRipError::CdText);
		}

		let cdtext_len = u16::from_be_bytes([buf[0], buf[1]])
			.checked_add(2) // Length excludes the 2-byte length field itself.
			.ok_or(RipRipError::CdText)?;

		if cdtext_len as usize == TOC_HEADER_LEN {
			return Ok(None); // No CD-Text exists on this disc.
		}

		// Patch the CDB with the expected allocation length.
		let mut cdb = CDB;
		copy_bytes(&mut cdb, 7..9, cdtext_len.to_be_bytes());

		let mut buf = vec![0u8; cdtext_len.into()];
		self.submit(&cdb, &mut buf)?;

		Ok(Some(buf))
	}

	fn get_toc_header(&self) -> Result<(u8, u8), RipRipError> {
		const ALLOC_LEN: usize = TOC_HEADER_LEN;

		const CDB: [u8; 10] = {
			let mut cdb = [0; _];
			cdb[0] = READ_TOC;
			cdb[1] = FORMAT_LBA;
			cdb[2] = TOC_FORMAT_TOC; // Format 0: Standard Table of Contents.
			cdb[6] = 0;

			copy_bytes(&mut cdb, 7..9, to_be_u16(ALLOC_LEN));
			cdb
		};

		let mut buf = [0u8; ALLOC_LEN];
		if self.submit(&CDB, &mut buf)? < ALLOC_LEN {
			return Err(RipRipError::FirstTrackNum);
		}

		let first_track = buf[2];
		let last_track = buf[3];

		Ok((first_track, last_track))
	}

	fn get_track_descriptor(&self, idx: u8) -> Result<(u8, u32), RipRipError> {
		const ALLOC_LEN: usize = TOC_HEADER_LEN + TOC_TRACK_DESCRIPTOR_LEN;

		const CDB: [u8; 10] = {
			let mut cdb = [0; _];
			cdb[0] = READ_TOC;
			cdb[1] = FORMAT_LBA;
			cdb[2] = TOC_FORMAT_TOC; // Format 0: Standard Table of Contents.

			copy_bytes(&mut cdb, 7..9, to_be_u16(ALLOC_LEN));
			cdb
		};

		let mut cdb = CDB;
		cdb[6] = idx;

		let mut buf = [0u8; ALLOC_LEN];
		if self.submit(&cdb, &mut buf)? < ALLOC_LEN {
			return Err(RipRipError::TrackLba(idx));
		}

		let control_adr = buf[5];
		let lba = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);

		Ok((control_adr, lba))
	}

	fn drive_vendor_model__(&self) -> Result<DriveVendorModel, RipRipError> {
		use spc::{
			INQUIRY_HEADER_LEN, INQUIRY_PRODUCT_ID_LEN, INQUIRY_REVISION_LEVEL_LEN,
			INQUIRY_VENDOR_ID_LEN,
		};

		const VENDOR_ID_RANGE: std::ops::Range<usize> =
			INQUIRY_HEADER_LEN..(INQUIRY_HEADER_LEN + INQUIRY_VENDOR_ID_LEN);

		const PRODUCT_ID_RANGE: std::ops::Range<usize> =
			VENDOR_ID_RANGE.end..VENDOR_ID_RANGE.end + INQUIRY_PRODUCT_ID_LEN;

		const ALLOC_LEN: usize = INQUIRY_HEADER_LEN
			+ INQUIRY_VENDOR_ID_LEN
			+ INQUIRY_PRODUCT_ID_LEN
			+ INQUIRY_REVISION_LEVEL_LEN;

		const CDB: [u8; 6] = {
			let mut cdb = [0; _];
			cdb[0] = spc::INQUIRY;
			copy_bytes(&mut cdb, 4..5, to_be_u8(ALLOC_LEN));
			cdb
		};

		let mut buf = [0u8; ALLOC_LEN];
		if self.submit(&CDB, &mut buf)? < ALLOC_LEN {
			return Err(RipRipError::DriveModel);
		}

		let vendor_id = &buf[VENDOR_ID_RANGE];
		let model_id = &buf[PRODUCT_ID_RANGE];
		let revision_level = &buf[PRODUCT_ID_RANGE.end..];

		if let Ok(revision_level_str) = std::str::from_utf8(revision_level) {
			log!(@debug "Drive revision: {revision_level_str}.");
		}

		// Convert the raw bytes into UTF-8 strings.
		let vendor_id = std::str::from_utf8(vendor_id).map_err(|_| RipRipError::DriveVendor)?;
		let model_id = std::str::from_utf8(model_id).map_err(|_| RipRipError::DriveModel)?;

		DriveVendorModel::new(vendor_id, model_id)
	}

	fn read_cd__(&self, buf: &mut [u8], lsn: i32, c2: bool, sub: u8) -> Result<usize, RipRipError> {
		const CDB: [u8; 12] = {
			let mut cdb = [0; _];
			cdb[0] = READ_CD;
			cdb[1] = SECTOR_TYPE_CDDA;

			// Transfer Length (24-bit BE integer): one sector.
			copy_bytes(&mut cdb, 6..9, [0, 0, 1]);
			cdb
		};

		let mut cdb = CDB;
		// Rip Rip's addressing parameters are already absolute LBAs.
		let lba = u32::try_from(lsn).map_err(|_| RipRipError::CdRead)?;
		copy_bytes(&mut cdb, 2..6, lba.to_be_bytes());

		// Byte 9 is the Selection Field flag byte:
		// Bit 4: User Data Selection (Set to 1 to read the 2352 bytes audio payload)
		// Bit 2..1: C2 Error Flag selection allocation (Set to 1 to include 294 bytes C2 space)
		let user_data_flag = 0x10;
		let c2_flag = if c2 { 0x02 } else { 0x00 };
		cdb[9] = user_data_flag | c2_flag;

		// Byte 10 defines the Subchannel Selection configuration flags:
		// 0 = none, 1 = raw P-W, 2 = formatted Q, 4 = corrected R-W.
		cdb[10] = sub;

		self.submit(&cdb, buf)
	}
}

impl<T: MmcDriverExt> CddaDriverExt for T {
	fn first_track_num(&self) -> Result<u8, RipRipError> {
		let (first, _) = self.get_toc_header()?;

		if first == 0 {
			Err(RipRipError::FirstTrackNum)
		} else {
			Ok(first)
		}
	}

	fn leadout_lba(&self) -> Result<u32, RipRipError> {
		// In the SCSI MMC specification, the leadout track information is
		// explicitly queried using the standard magic track index 0xAA.
		self.track_lba_start(CD_LEADOUT)
	}

	fn num_tracks(&self) -> Result<u8, RipRipError> {
		let (first, last) = self.get_toc_header()?;

		if last == 0 {
			Err(RipRipError::NumTracks)
		} else {
			// Handles discs that might not explicitly start at track 1
			Ok(last - first + 1)
		}
	}

	fn track_format(&self, idx: u8) -> Result<bool, RipRipError> {
		let (control_adr, _) = self
			.get_track_descriptor(idx)
			.map_err(|_| RipRipError::TrackFormat(idx))?;

		// In SCSI MMC TOC structures, the 4-bit CONTROL field dictates data types.
		// Bit 2 (0x04) is set if the track is a data track, and clear if it's audio.
		let is_data = (control_adr & CTRL_DATA_TRACK) > 0;

		Ok(!is_data)
	}

	fn track_lba_start(&self, idx: u8) -> Result<u32, RipRipError> {
		if idx == 0 {
			return Err(RipRipError::TrackNumber(0));
		}

		let (_, lba) = self
			.get_track_descriptor(idx)
			.map_err(|_| RipRipError::TrackLba(idx))?;

		Ok(lba + u32::from(CD_LEADIN))
	}

	fn cdtext(&self) -> Option<Vec<u8>> {
		if let Some(mut buf) = self.read_cdtext().ok()? {
			// Skip the header.
			buf.copy_within(TOC_HEADER_LEN.., 0);
			buf.truncate(buf.len() - TOC_HEADER_LEN);
			return Some(buf);
		}
		None
	}

	fn drive_vendor_model(&self) -> Option<DriveVendorModel> {
		self.drive_vendor_model__().ok()
	}

	fn mcn_subchannel(&self) -> Option<Barcode> {
		self.mcn_subchannel__().ok().flatten()
	}

	fn read_cd(
		&self,
		buf: &mut [u8],
		lsn: i32,
		c2: bool,
		sub: u8,
		_block_size: u16,
	) -> Result<(), RipRipError> {
		if self.read_cd__(buf, lsn, c2, sub).is_ok() {
			Ok(())
		} else {
			crate::drivers::set_bad_sector(lsn);
			Err(RipRipError::CdRead)
		}
	}
}
