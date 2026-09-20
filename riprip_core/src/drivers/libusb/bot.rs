/*!
# Rip Rip Hooray: Bulk-Only Transport

Provides the `CommandBlockWrapper` and `CommandStatusWrapper` structures
required to transport SCSI MMC commands over USB.
*/

/// # Mass Storage Class.
pub(super) const CLASS_MASS_STORAGE: u8 = 0x08;

/// # Bulk-Only Protocol.
pub(super) const PROTOCOL_BULK_ONLY: u8 = 0x50;

/// # Device Subclass: CD-ROM.
const SUBCLASS_CD_ROM: u8 = 0x02;

/// # Device Subclass: Legacy/ATAPI CD-Rom.
const SUBCLASS_SFF_8070I: u8 = 0x05;

/// # Device Subclass: Modern USB-SATA Bridges.
const SUBCLASS_SCSI_TRANSPARENT: u8 = 0x06;

/// # Device Subclasses of Interest.
///
/// Interfaces with these sublcasses are worth a deeper look as they might
/// be CD-ROMs to rip from.
pub(super) const OPTICAL_DRIVE_SUBCLASSES: [u8; 3] = [
	SUBCLASS_CD_ROM,
	SUBCLASS_SFF_8070I,
	SUBCLASS_SCSI_TRANSPARENT,
];

/// # Command Block Wrapper Length.
const CBW_LEN: usize = 31;

/// # Command Status Wrapper Length.
pub(super) const CSW_LEN: usize = 13;

/// # Command Block Wrapper Signature.
const CBW_SIGNATURE: u32 = u32::from_le_bytes(*b"USBC");

/// # Command Status Wrapper Signature.
const CSW_SIGNATURE: u32 = u32::from_le_bytes(*b"USBS");

#[derive(Debug, Default)]
/// # Command Block.
pub(super) struct CommandBlockWrapper {
	/// # Signature.
	signature: u32,

	/// # Tag.
	tag: u32,

	/// # Transfer Length.
	data_transfer_length: u32,

	/// # Flags.
	flags: u8,

	/// # TODO.
	lun: u8,

	/// # Command Block Length.
	cb_length: u8,

	/// # Command Descriptor Block.
	cdb: [u8; 16],
}

impl CommandBlockWrapper {
	#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
	#[must_use]
	/// # New.
	pub(super) fn new<const N: usize>(
		tag: u32,
		data_transfer_length: u32,
		flags: u8,
		lun: u8,
		cdb: &[u8; N],
	) -> Self {
		const {
			assert!(N <= 16, "BUG: CDB cannot exceed 16 bytes.");
		}

		Self {
			signature: CBW_SIGNATURE,
			tag,
			data_transfer_length,
			flags,
			lun,
			cb_length: N as u8,
			cdb: std::array::from_fn(|i| cdb.get(i).copied().unwrap_or(0)),
		}
	}

	#[must_use]
	/// # To Bytes.
	pub(super) fn to_bytes(&self) -> [u8; CBW_LEN] {
		let mut buf = [0_u8; CBW_LEN];
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
/// # Command Status.
pub(super) struct CommandStatusWrapper {
	/// # Signature.
	signature: u32,

	/// # Tag.
	tag: u32,

	#[expect(dead_code, reason = "We might want this some day.")]
	/// # Residue.
	data_residue: u32,

	/// # Status.
	status: u8,
}

impl CommandStatusWrapper {
	#[must_use]
	/// # From Bytes.
	pub(super) const fn from_bytes(buf: &[u8; CSW_LEN]) -> Self {
		Self {
			signature: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
			tag: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
			data_residue: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
			status: buf[12],
		}
	}

	#[must_use]
	/// # Is Valid?
	pub(super) const fn is_valid(&self, tag: u32) -> bool {
		self.signature == CSW_SIGNATURE && self.tag == tag
	}

	#[must_use]
	/// # Status
	pub(super) const fn status(&self) -> u8 { self.status }

	#[expect(dead_code, reason = "We might want this some day.")]
	#[must_use]
	/// # Data Residue.
	pub(super) const fn data_residue(&self) -> u32 { self.data_residue }
}
