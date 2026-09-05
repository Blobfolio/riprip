/*!
# Rip Rip Hooray: SCSI Multimedia Commands

Provides opcodes, profiles, and format constants for SCSI MMC and core SPC commands
used to inspect and read audio CDs.
*/

// Opcodes
const READ_SUB_CHANNEL: u8 = 0x42;
const READ_TOC: u8 = 0x43;
const GET_CONFIGURATION: u8 = 0x46;
const READ_CD: u8 = 0xBE;

// Architectural Constraints
const FIRST_TRACK: u8 = 0x01;
const MAX_TRACK_NUMBER: u8 = 99;
pub(super) const LEAD_OUT: u8 = 0xAA;

// READ_SUB_CHANNEL Data Formats
const SUB_FORMAT_MCN: u8 = 0x02;
const SUB_FORMAT_ISRC: u8 = 0x03;

// READ_TOC Time/Address Format
const FORMAT_LBA: u8 = 0x00;
const FORMAT_MSF: u8 = 0x02;

// READ_TOC Format Codes
const TOC_FORMAT_TOC: u8 = 0x00;
const TOC_FORMAT_SESSION: u8 = 0x01;
const TOC_FORMAT_FULL: u8 = 0x02;
const TOC_FORMAT_PMA: u8 = 0x03;
const TOC_FORMAT_ATIP: u8 = 0x04;
const TOC_FORMAT_CDTEXT: u8 = 0x05;

const TOC_HEADER_LEN: usize = 4;
const TOC_TRACK_DESCRIPTOR_LEN: usize = 8;

pub(super) const CTRL_DATA_TRACK: u8 = 0x04; // Bitmask for track type: set = Data, cleared = Audio.

// GET_CONFIGURATION Features
const FEATURE_CD_AUDIO_C2: u16 = 0x001E;

const PROFILE_CD_ROM: u16 = 0x0008; // Read-only pressed CD.
const PROFILE_CD_R: u16 = 0x0009; // Write-once CD-Recordable.
const PROFILE_CD_RW: u16 = 0x000A; // Rewritable CD.

// READ_CD Sector Types
const SECTOR_TYPE_CDDA: u8 = 0x04;

/// `spc` (SCSI Primary Commands) covers baseline commands that every SCSI device must understand,
/// regardless of what it is (like INQUIRY or TEST_UNIT_READY).
mod spc {
    pub(super) const TEST_UNIT_READY: u8 = 0x00;
    pub(super) const REQUEST_SENSE: u8 = 0x03;
    pub(super) const INQUIRY: u8 = 0x12;
    pub(super) const MODE_SELECT_10: u8 = 0x55;
    pub(super) const MODE_SENSE_10: u8 = 0x5A;
}

use crate::{Barcode, DriveVendorModel, RipRipError};

pub(super) trait Transport {
    /// Sends a SCSI Command Descriptor Block (CDB) and transfers data from the device.
    fn submit<const N: usize>(&self, cdb: &[u8; N], data: &mut [u8]) -> Result<usize, RipRipError>;
}

/// A low-level API for interacting with a physical CD drive specifically to read audio data.
///
/// Relies on the underlying `Transport` trait to handle the hardware bus communication
/// (e.g. USB BOT or `/dev/sg`).
pub(super) trait Drive: Transport {
    fn mcn_subchannel__(&self) -> Option<Barcode> {
        // Request 26 bytes (Standard Sub-channel header + MCN data block size).
        const ALLOC_LEN: u16 = 26;

        let mut cdb = [0u8; 10];
        cdb[0] = READ_SUB_CHANNEL;
        cdb[1] = FORMAT_MSF;
        cdb[2] = 0x40; // Sub-Q Channel tracking bit
        cdb[3] = SUB_FORMAT_MCN;

        cdb[7..9].copy_from_slice(&ALLOC_LEN.to_be_bytes());

        let mut buf = [0u8; ALLOC_LEN as usize];
        self.submit(&cdb, &mut buf).ok()?;

        let data_format = buf[3];
        let subq_element_valid = buf[4];

        if data_format == SUB_FORMAT_MCN && subq_element_valid == 0x01 {
            // Bit 7 tracks string validation rules (MCVAL flag in MMC spec).
            let is_mcn_valid = (buf[12] & 0x80) != 0;
            if is_mcn_valid {
                let raw_ascii = &buf[13..26];
                return Barcode::try_from(raw_ascii).ok();
            }
        }
        None
    }

    fn check_disc_mode__(&self) -> Result<(), RipRipError> {
        let mut buf = vec![];

        let read_toc = |alloc_len: u16, buf: &mut Vec<u8>| -> Result<usize, RipRipError> {
            let mut cdb = [0u8; 10];
            cdb[0] = READ_TOC;
            cdb[1] = FORMAT_LBA;
            cdb[2] = TOC_FORMAT_TOC; // Format 0: Standard Table of Contents.
            cdb[6] = FIRST_TRACK; // Starting track.

            cdb[7..9].copy_from_slice(&alloc_len.to_be_bytes());

            buf.clear();
            buf.resize(alloc_len as usize, 0);
            
            self.submit(&cdb, buf).map_err(|_| RipRipError::DiscMode)
        };

        // Asks only for enough bytes to discover how large the TOC is.
        let len = read_toc(TOC_HEADER_LEN as u16, &mut buf)?;
        if len < TOC_HEADER_LEN {
            return Err(RipRipError::DiscMode);
        }

        let toc_len = u16::from_be_bytes([buf[0], buf[1]])
            .checked_add(2) // Length excludes the 2-byte length field itself.
            .ok_or(RipRipError::DiscMode)?;
        
        let len = read_toc(toc_len, &mut buf)?;
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
            .chunks_exact(TOC_TRACK_DESCRIPTOR_LEN)
            .take(track_count)
            .any(|desc| (desc[1] & CTRL_DATA_TRACK) == 0);

        if has_audio {
            Ok(())
        } else {
            Err(RipRipError::DiscMode)
        }
    }

    fn check_c2__(&self) -> Result<(), RipRipError> {
        // Request header (8 bytes) + feature descriptor payload (8 bytes).
        const ALLOC_LEN: u16 = 16;

        let mut cdb = [0u8; 10];
        cdb[0] = GET_CONFIGURATION;
        cdb[1] = 0x02; // RT field = 0x02: Request only the specific feature specified in bytes 2-3.
        cdb[2..4].copy_from_slice(&FEATURE_CD_AUDIO_C2.to_be_bytes());

        cdb[7..9].copy_from_slice(&ALLOC_LEN.to_be_bytes());

        let mut buf = [0u8; ALLOC_LEN as usize];
        if self.submit(&cdb, &mut buf).is_err() {
            return Err(RipRipError::C2Mode296);
        }

        let desc = &buf[8..16];

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
        let mut buf = vec![];

        let read_toc = |alloc_len: u16, buf: &mut Vec<u8>| -> Result<usize, RipRipError> {
            let mut cdb = [0u8; 10];
            cdb[0] = READ_TOC;
            cdb[1] = FORMAT_LBA;
            cdb[2] = TOC_FORMAT_CDTEXT;
            cdb[6] = 0; // Track number to start reading from (0 = entire disc).

            cdb[7..9].copy_from_slice(&alloc_len.to_be_bytes());

            buf.clear();
            buf.resize(alloc_len as usize, 0);
            
            self.submit(&cdb, buf).map_err(|_| RipRipError::DiscMode)
        };

        // Asks only for enough bytes to discover how large the CD-Text is.
        let len = read_toc(TOC_HEADER_LEN as u16, &mut buf)?;
        if len < TOC_HEADER_LEN {
            return Err(RipRipError::DiscMode);
        }

        let cdtext_len = u16::from_be_bytes([buf[0], buf[1]])
            .checked_add(2) // Length excludes the 2-byte length field itself.
            .ok_or(RipRipError::DiscMode)?;

        if cdtext_len as usize == TOC_HEADER_LEN {
            return Ok(None); // No CD-Text exists on this disc.
        }
        
        read_toc(cdtext_len, &mut buf)?;

        Ok(Some(buf))
    }

    fn get_toc_header(&self) -> Result<(u8, u8), RipRipError> {
        const ALLOC_LEN: u16 = TOC_HEADER_LEN as u16;

        let mut cdb = [0u8; 10];
        cdb[0] = READ_TOC;
        cdb[1] = FORMAT_LBA;
        cdb[2] = TOC_FORMAT_TOC; // Format 0: Standard Table of Contents.
        cdb[6] = 0;

        cdb[7..9].copy_from_slice(&ALLOC_LEN.to_be_bytes());

        let mut buf = [0u8; ALLOC_LEN as usize];
        let len = self.submit(&cdb, &mut buf)?;
        if len < TOC_HEADER_LEN {
            return Err(RipRipError::FirstTrackNum);
        }

        let first_track = buf[2];
        let last_track = buf[3];

        Ok((first_track, last_track))
    }

    fn get_track_descriptor(&self, idx: u8) -> Result<(u8, u32), RipRipError> {
        const ALLOC_LEN: usize = TOC_HEADER_LEN + TOC_TRACK_DESCRIPTOR_LEN;
        const _: () = assert!(ALLOC_LEN <= u16::MAX as usize);

        let mut cdb = [0u8; 10];
        cdb[0] = READ_TOC;
        cdb[1] = FORMAT_LBA;
        cdb[2] = TOC_FORMAT_TOC; // Format 0: Standard Table of Contents.
        cdb[6] = idx;

        cdb[7..9].copy_from_slice(&ALLOC_LEN.to_be_bytes());

        let mut buf = [0u8; ALLOC_LEN];
        if self.submit(&cdb, &mut buf)? < ALLOC_LEN {
            return Err(RipRipError::TrackLba(idx));
        }

        let control_adr = buf[5];
        let lba = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);

        Ok((control_adr, lba))
    }

    fn drive_vendor_model__(&self) -> Option<DriveVendorModel> {
        // Allocation Length: Standard INQUIRY data size is 36 bytes.
        const ALLOC_LEN: u8 = 36;

        let mut cdb = [0u8; 6];
        cdb[0] = spc::INQUIRY;
        cdb[4] = ALLOC_LEN;

        let mut buf = [0u8; ALLOC_LEN as usize];
        self.submit(&cdb, &mut buf).ok()?;

        // Standard SCSI Inquiry layout maps fields at fixed offsets:
        // Bytes 8..16  -> Vendor Identification (8 bytes)
        // Bytes 16..32 -> Product Identification / Model (16 bytes)
        let vendor_raw = &buf[8..16];
        let model_raw = &buf[16..32];

        // Convert the raw bytes into UTF-8 strings, stripping away any
        // trailing whitespace padding added by the drive firmware.
        let vendor_str = std::str::from_utf8(vendor_raw).ok()?.trim();
        let model_str = std::str::from_utf8(model_raw).ok()?.trim();

        // Model is required, Vendor might be empty strings.
        if model_str.is_empty() {
            return None;
        }

        DriveVendorModel::new(vendor_str, model_str).ok()
    }

    fn read_cd__(&self, buf: &mut [u8], lsn: i32, c2: bool, sub: u8) -> Result<usize, RipRipError> {
        let mut cdb = [0u8; 12];
        cdb[0] = READ_CD;
        cdb[1] = SECTOR_TYPE_CDDA;

        // Rip Rip's addressing parameters are already absolute LBAs.
        let lba = lsn as u32;
        cdb[2..6].copy_from_slice(&lba.to_be_bytes());

        // Transfer Length is a 24-bit BE integer spanning bytes 6, 7, and 8.
        // Since we only ever read 1 sector, bytes 6 and 7 remain 0, and byte 8 is 1.
        cdb[8] = 1;

        // Byte 9 is the Selection Field flag byte:
        // Bit 4: User Data Selection (Set to 1 to read the 2352 bytes audio payload)
        // Bit 2..1: C2 Error Flag selection allocation (0x02 means include 294 bytes C2 space)
        let user_data_flag = 0x10;
        let c2_flag = if c2 { 0x02 } else { 0x00 };
        cdb[9] = user_data_flag | c2_flag;

        // Byte 10 defines the Sub-channel Selection configuration flags:
        // 0x00 = No sub-channel data requested
        // 0x02 = Raw Subchannel Data payload (16 bytes payload space)
        cdb[10] = sub;

        self.submit(&cdb, buf)
    }
}
