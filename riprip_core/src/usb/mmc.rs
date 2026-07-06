/*!
# Rip Rip Hooray: SCSI Multimedia Commands (`mmc`)

Provides opcodes, profiles, and format constants for SCSI MMC and core SPC commands
used to inspect and read audio CDs.
*/

// SCSI Opcodes
pub(super) const READ_SUB_CHANNEL: u8 = 0x42;
pub(super) const READ_TOC: u8 = 0x43;
pub(super) const GET_CONFIGURATION: u8 = 0x46;
pub(super) const READ_CD: u8 = 0xBE;

// Architectural Constraints
pub(super) const FIRST_TRACK: u8 = 0x01;
pub(super) const LEAD_OUT: u8 = 0xAA;

// READ_TOC Time/Address Format
pub(super) const FORMAT_LBA: u8 = 0x00;
pub(super) const FORMAT_MSF: u8 = 0x02;

// READ_TOC Format Codes
pub(super) const TOC_FORMAT_TOC: u8 = 0x00;
pub(super) const TOC_FORMAT_SESSION: u8 = 0x01;
pub(super) const TOC_FORMAT_FULL: u8 = 0x02;
pub(super) const TOC_FORMAT_PMA: u8 = 0x03;
pub(super) const TOC_FORMAT_ATIP: u8 = 0x04;
pub(super) const TOC_FORMAT_CDTEXT: u8 = 0x05;

pub(super) const CTRL_DATA_TRACK: u8 = 0x04; // Bitmask for track type: set = Data, cleared = Audio.

// READ_SUB_CHANNEL Data Formats
pub(super) const SUB_FORMAT_MCN: u8 = 0x02;
pub(super) const SUB_FORMAT_ISRC: u8 = 0x03;

pub(super) const PROFILE_CD_ROM: u16 = 0x0008; // Read-only pressed CD.
pub(super) const PROFILE_CD_R: u16 = 0x0009; // Write-once CD-Recordable.
pub(super) const PROFILE_CD_RW: u16 = 0x000A; // Rewritable CD.

/// `spc` (SCSI Primary Commands) covers baseline commands that every SCSI device must understand,
/// regardless of what it is (like INQUIRY or TEST_UNIT_READY).
pub(super) mod spc {
    pub(crate) const TEST_UNIT_READY: u8 = 0x00;
    pub(crate) const REQUEST_SENSE: u8 = 0x03;
    pub(crate) const INQUIRY: u8 = 0x12;
    pub(crate) const MODE_SELECT_10: u8 = 0x55;
    pub(crate) const MODE_SENSE_10: u8 = 0x5A;
}
