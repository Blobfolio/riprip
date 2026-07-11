/*!
# Rip Rip Hooray: Bulk-Only Transport

Provides the `CommandBlockWrapper` and `CommandStatusWrapper` structures required to transport
SCSI MMC commands over USB.
*/

pub(super) const CLASS_MASS_STORAGE: u8 = 0x08;

pub(super) const PROTOCOL_BULK_ONLY: u8 = 0x50;

pub(super) const SUBCLASS_CD_ROM: u8 = 0x02;
pub(super) const SUBCLASS_SFF_8070I: u8 = 0x05; // Often used for legacy/ATAPI CD-ROMs.
pub(super) const SUBCLASS_SCSI_TRANSPARENT: u8 = 0x06; // Common for modern USB-SATA bridges.

pub(super) const OPTICAL_DRIVE_SUBCLASSES: [u8; 3] = [
    SUBCLASS_CD_ROM,
    SUBCLASS_SFF_8070I,
    SUBCLASS_SCSI_TRANSPARENT,
];

pub(super) const CBW_SIGNATURE: u32 = u32::from_le_bytes(*b"USBC");
pub(super) const CSW_SIGNATURE: u32 = u32::from_le_bytes(*b"USBS");
pub(super) const CBW_LEN: usize = 31;
pub(super) const CSW_LEN: usize = 13;

#[derive(Debug, Default)]
pub(super) struct CommandBlockWrapper {
    pub signature: u32,
    pub tag: u32,
    pub data_transfer_length: u32,
    pub flags: u8,
    pub lun: u8,
    pub cb_length: u8,
    pub cdb: [u8; 16],
}

impl CommandBlockWrapper {
    pub(super) fn to_bytes(&self) -> [u8; CBW_LEN] {
        let mut buf = [0u8; CBW_LEN];
        buf[0..4].copy_from_slice(&self.signature.to_le_bytes());
        buf[4..8].copy_from_slice(&self.tag.to_le_bytes());
        buf[8..12].copy_from_slice(&self.data_transfer_length.to_le_bytes());
        buf[12] = self.flags;
        buf[13] = self.lun;
        buf[14] = self.cb_length;
        buf[15..31].copy_from_slice(&self.cdb);
        buf
    }
}

#[derive(Debug, Default)]
pub(super) struct CommandStatusWrapper {
    pub signature: u32,
    pub tag: u32,
    pub data_residue: u32,
    pub status: u8,
}

impl CommandStatusWrapper {
    pub(super) fn from_bytes(buf: &[u8; CSW_LEN]) -> Self {
        Self {
            signature: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            tag: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            data_residue: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            status: buf[12],
        }
    }

    pub(super) fn is_valid(&self, tag: u32) -> bool {
        self.signature == CSW_SIGNATURE && self.tag == tag 
    }
}
