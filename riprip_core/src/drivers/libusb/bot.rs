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

pub(super) const CBW_LEN: usize = 31;
pub(super) const CSW_LEN: usize = 13;

const CBW_SIGNATURE: u32 = u32::from_le_bytes(*b"USBC");
const CSW_SIGNATURE: u32 = u32::from_le_bytes(*b"USBS");

#[derive(Debug, Default)]
pub(super) struct CommandBlockWrapper {
	signature: u32,
	tag: u32,
	data_transfer_length: u32,
	flags: u8,
	lun: u8,
	cb_length: u8,
	cdb: [u8; 16],
}

impl CommandBlockWrapper {
	pub(super) fn new<const N: usize>(
		tag: u32,
		data_transfer_length: u32,
		flags: u8,
		lun: u8,
		cdb: &[u8; N],
	) -> Self {
		const { assert!(N <= 16, "CDB cannot exceed 16 bytes.") };

		let mut cbw = Self {
			signature: CBW_SIGNATURE,
			tag,
			data_transfer_length,
			flags,
			lun,
			cb_length: N as u8,
			cdb: [0; 16],
		};

		cbw.cdb[..N].copy_from_slice(cdb);
		cbw
	}

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
	signature: u32,
	tag: u32,
	data_residue: u32,
	status: u8,
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

	pub(super) fn status(&self) -> u8 {
		self.status
	}

	pub(super) fn data_residue(&self) -> u32 {
		self.data_residue
	}
}
