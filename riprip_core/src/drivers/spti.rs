/*!
# Rip Rip Hooray: Windows SCSI Pass-Through Interface.

Sends the shared MMC commands through the Windows CD-ROM class driver.
See <https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddscsi/ni-ntddscsi-ioctl_scsi_pass_through_direct>.
*/

use crate::{
	CddaDriverNewExt,
	macros::log,
	RipRipError,
};
use super::{
	mmc::{
		CommandDescriptorBlock,
		MmcDriverExt,
		TransportExt,
	},
	open_err,
};
use std::{
	fs::{
		File,
		OpenOptions,
	},
	mem::{
		offset_of,
		size_of,
	},
	num::NonZeroUsize,
	os::windows::{
		fs::OpenOptionsExt,
		io::AsRawHandle,
	},
	path::{
		Path,
		PathBuf,
	},
};
use windows_sys::Win32::{
	Storage::{
		FileSystem::{
			FILE_SHARE_READ,
			FILE_SHARE_WRITE,
			GetDriveTypeW,
			GetLogicalDrives,
		},
		IscsiDisc::{
			IOCTL_SCSI_PASS_THROUGH_DIRECT,
			SCSI_PASS_THROUGH_DIRECT,
		},
	},
	System::{
		IO::DeviceIoControl,
		Ioctl::{
			IOCTL_STORAGE_QUERY_PROPERTY,
			PropertyStandardQuery,
			STORAGE_ADAPTER_DESCRIPTOR,
			STORAGE_PROPERTY_QUERY,
			StorageAdapterProperty,
		},
		WindowsProgramming::DRIVE_CDROM,
	},
};

/// # Maximum Request Sense Bytes.
const SENSE_LEN: u8 = 32;

#[repr(C)]
/// # Pass-through Header and Contiguous Sense Buffer.
struct Request {
	/// # Native SDK Header.
	header: SCSI_PASS_THROUGH_DIRECT,

	/// # Sense Data Returned on CHECK CONDITION.
	sense: [u8; SENSE_LEN as usize],
}

/// # Windows Optical Drive.
pub(crate) struct SptiInstance {
	/// # CD-ROM Class Device Handle.
	device: File,

	/// # Required Data Buffer Alignment (Mask + 1).
	alignment: NonZeroUsize,
}

impl CddaDriverNewExt for SptiInstance {
	/// # New!
	fn new(dev: Option<PathBuf>) -> Result<Self, RipRipError> {
		log!(@debug "Initializing Windows SPTI driver.");
		if let Some(dev) = dev {
			let path = device_path(&dev).ok_or_else(|| open_err(Some(&dev)))?;
			return Self::open(&path);
		}

		// Prefer the first usable audio disc, skipping empty or data-only drives.
		let mut error = open_err(None);
		for path in optical_drives() {
			match Self::open(&path) {
				Ok(drive) => return Ok(drive),
				Err(e) => { error = e; },
			}
		}
		Err(error)
	}
}

impl SptiInstance {
	/// # Open and Validate an Optical Drive.
	fn open(dev: &Path) -> Result<Self, RipRipError> {
		let device = OpenOptions::new()
			.read(true)
			.write(true)
			.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
			.open(dev)
			.map_err(|e| {
				log!(@trace [dev] "Failed to open device: {e}.");
				open_err(Some(dev))
			})?;
		let alignment = adapter_alignment(&device)?;
		let out = Self { device, alignment };
		out.check_disc_mode__()?;
		out.check_c2__()?;
		Ok(out)
	}
}

impl TransportExt for SptiInstance {
	#[expect(unsafe_code, reason = "Synchronous Windows device I/O.")]
	#[expect(clippy::cast_possible_truncation, reason = "Fixed SDK structure sizes fit u16/u32.")]
	fn submit(
		&self,
		cdb: &CommandDescriptorBlock,
		buf: &mut [u8],
		ctx: &'static str,
	) -> Result<usize, RipRipError> {
		let length = u32::try_from(buf.len())
			.map_err(|_| RipRipError::Bug("Transfer length exceeds u32."))?;

		// A separate aligned buffer accommodates arbitrary caller slices and
		// prevents a failed command from changing recovery data.
		let mut storage = vec![0_u8; buf.len() + self.alignment.get() - 1];

		let offset = storage.as_ptr().align_offset(self.alignment.get());
		let data = &mut storage[offset..offset + buf.len()];
		let mut request = Request {
			header: SCSI_PASS_THROUGH_DIRECT {
				Length: size_of::<SCSI_PASS_THROUGH_DIRECT>() as u16,
				CdbLength: cdb.len(),
				SenseInfoLength: SENSE_LEN,
				DataIn: if buf.is_empty() { 2 } else { 1 },
				DataTransferLength: length,
				TimeOutValue: 10,
				DataBuffer: if buf.is_empty() { std::ptr::null_mut() } else { data.as_mut_ptr().cast() },
				SenseInfoOffset: offset_of!(Request, sense) as u32,
				..SCSI_PASS_THROUGH_DIRECT::default()
			},
			sense: [0; SENSE_LEN as usize],
		};
		request.header.Cdb[..usize::from(cdb.len())].copy_from_slice(cdb.as_slice());
		let request_len = size_of::<Request>() as u32;
		let mut returned = 0;
		let ptr = std::ptr::from_mut(&mut request).cast();
		// SAFETY: all pointers reference live, correctly sized buffers. The
		// data buffer meets the queried adapter alignment. The handle is owned
		// by self, and this synchronous call completes before any buffers drop.
		let success = unsafe {
			DeviceIoControl(
				self.device.as_raw_handle(),
				IOCTL_SCSI_PASS_THROUGH_DIRECT,
				ptr,
				request_len,
				ptr,
				request_len,
				&raw mut returned,
				std::ptr::null_mut(),
			)
		};

		if success == 0 {
			let error = std::io::Error::last_os_error();
			log!(@trace [ctx, cdb] "SPTI request failed: {error}.");
			return Err(RipRipError::Internal(ctx, error.to_string()));
		}

		if (returned as usize) < size_of::<SCSI_PASS_THROUGH_DIRECT>() {
			let expected = size_of::<SCSI_PASS_THROUGH_DIRECT>();
			log!(
				@trace [ctx, cdb, returned, expected]
				"Truncated SPTI response header.",
			);
			return Err(RipRipError::Internal(ctx, "Truncated SPTI response header.".into()));
		}

		// Check the SENSE status.
		let sense_len = usize::from(request.header.SenseInfoLength.min(SENSE_LEN))
			.min((returned as usize).saturating_sub(offset_of!(Request, sense)));
		check_status(cdb, request.header.ScsiStatus, &request.sense[..sense_len])?;

		// Check the transfer length.
		let transferred = request.header.DataTransferLength as usize;
		if buf.len() < transferred {
			log!(
				@trace [ctx, cdb, transferred, buf.len()]
				"SPTI transfer exceeds buffer length.",
			);
			return Err(RipRipError::Bug("SPTI transfer exceeds buffer length."));
		}

		// Copy to the buffer and return the amount written.
		buf[..transferred].copy_from_slice(&data[..transferred]);
		Ok(transferred)
	}
}

/// # Check Raw SCSI Status and Decode Readiness Sense.
fn check_status(cdb: &CommandDescriptorBlock, status: u8, sense: &[u8])
-> Result<(), RipRipError> {
	match status {
		0 => Ok(()),
		0x02 if cdb.is_test_unit_ready() => {
			let details = match sense {
				[response, _, key, _, _, _, _, _, _, _, _, _, asc, ascq, ..]
					if matches!(response & 0x7F, 0x70 | 0x71) => Some([key & 0x0F, *asc, *ascq]),
				[response, key, asc, ascq, ..]
					if matches!(response & 0x7F, 0x72 | 0x73) => Some([key & 0x0F, *asc, *ascq]),
				_ => None,
			};
			Err(RipRipError::TestUnitNotReady(details))
		},
		0x08 if cdb.is_test_unit_ready() => Err(RipRipError::TestUnitNotReady(None)),
		_ => {
			log!(@trace [cdb, status, sense] "SCSI command did not pass.");
			Err(RipRipError::CdRead)
		},
	}
}

#[expect(unsafe_code, reason = "Windows adapter property query.")]
#[expect(clippy::cast_possible_truncation, reason = "Fixed SDK structure sizes fit u32.")]
/// # Query Direct-I/O Alignment.
fn adapter_alignment(device: &File) -> Result<NonZeroUsize, RipRipError> {
	let query = STORAGE_PROPERTY_QUERY {
		PropertyId: StorageAdapterProperty,
		QueryType: PropertyStandardQuery,
		..STORAGE_PROPERTY_QUERY::default()
	};
	let mut descriptor = STORAGE_ADAPTER_DESCRIPTOR::default();
	let mut returned = 0;
	// SAFETY: input and output pointers are live SDK structures, their sizes
	// match the supplied lengths, and the device handle stays open.
	let success = unsafe {
		DeviceIoControl(
			device.as_raw_handle(),
			IOCTL_STORAGE_QUERY_PROPERTY,
			std::ptr::from_ref(&query).cast(), size_of::<STORAGE_PROPERTY_QUERY>() as u32,
			std::ptr::from_mut(&mut descriptor).cast(), size_of::<STORAGE_ADAPTER_DESCRIPTOR>() as u32,
			&raw mut returned,
			std::ptr::null_mut(),
		)
	};
	if success == 0 {
		return Err(RipRipError::Internal(
			"adapter_alignment",
			std::io::Error::last_os_error().to_string(),
		));
	}

	let required = offset_of!(STORAGE_ADAPTER_DESCRIPTOR, AlignmentMask) + size_of::<u32>();
	let alignment = NonZeroUsize::MIN.saturating_add(descriptor.AlignmentMask as usize);
	if
		(returned as usize) < required ||
		! alignment.is_power_of_two() ||
		usize::from(u16::MAX) < alignment.get()
	{
		return Err(RipRipError::Internal(
			"adapter_alignment",
			"Invalid storage adapter alignment.".into(),
		));
	}

	Ok(alignment)
}

/// # Normalize a Drive Letter or CD-ROM Device Name.
///
/// Only volume roots and CdRomN names are accepted; files and disk devices
/// are not valid optical-drive selections.
fn device_path(dev: &Path) -> Option<PathBuf> {
	let raw = dev.to_str()?;
	let name = raw.strip_prefix(r"\\.\").unwrap_or(raw);

	match name.as_bytes() {
		[letter, b':'] |
		[letter, b':', b'\\' | b'/'] if letter.is_ascii_alphabetic() =>  Some(PathBuf::from(format!(
			r"\\.\{}:",
			char::from(letter.to_ascii_uppercase())
		))),
		[b'C', b'd', b'R', b'o', b'm', rest @ ..] if rest.iter().all(u8::is_ascii_digit) => Some(PathBuf::from(format!(
			r"\\.\CdRom{}",
			oxford_join::join!(rest.iter().copied().map(char::from)),
		))),
		_ => None,
	}
}

#[expect(unsafe_code, reason = "Windows logical drive enumeration.")]
/// # Enumerate Optical Drive Letters.
fn optical_drives() -> Vec<PathBuf> {
	// SAFETY: GetLogicalDrives has no pointer arguments.
	let drives = unsafe { GetLogicalDrives() };
	(b'A'..=b'Z').filter_map(|letter| {
		if drives & (1_u32 << (letter - b'A')) == 0 { return None; }
		let root = [u16::from(letter), u16::from(b':'), u16::from(b'\\'), 0];
		// SAFETY: root is a live, null-terminated UTF-16 drive root.
		if unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_CDROM {
			Some(PathBuf::from(format!(r"\\.\{}:", char::from(letter))))
		}
		else { None }
	}).collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn device_names() {
		for name in [
			"d:",
			"D:",
			"d:/",
			"D:/",
			r"d:\",
			r"D:\",
			r"\\.\d:",
			r"\\.\D:",
			r"\\.\d:/",
			r"\\.\D:/",
			r"\\.\d:\",
			r"\\.\D:\",
		] {
			assert_eq!(
				device_path(Path::new(name)),
				Some(PathBuf::from(r"\\.\D:")),
			);
		}

		assert_eq!(
			device_path(Path::new(r"\\.\cdrom12")),
			Some(PathBuf::from(r"\\.\CdRom12")),
		);
		for name in [
			"",
			"D",
			"1:",
			"D:relative",
			r"D:\file",
			r"\\.\PhysicalDrive0",
			r"\\server\share",
			"CdRom",
			"CdRom-1",
		] {
			assert_eq!(device_path(Path::new(name)), None, "accepted {name}");
		}
	}

	#[test]
	fn scsi_readiness() {
		let ready = CommandDescriptorBlock::Six([0; 6]);
		assert!(check_status(&ready, 0, &[]).is_ok());
		let mut sense = [0; 18];
		sense[0] = 0xF0;
		sense[2] = 2;
		sense[12] = 4;
		sense[13] = 1;
		assert!(matches!(
			check_status(&ready, 2, &sense),
			Err(RipRipError::TestUnitNotReady(Some([2, 4, 1]))),
		));
		assert!(matches!(
			check_status(&ready, 2, &[0x72, 6, 0x28, 0]),
			Err(RipRipError::TestUnitNotReady(Some([6, 0x28, 0]))),
		));
		assert!(matches!(
			check_status(&ready, 2, &[]),
			Err(RipRipError::TestUnitNotReady(None)),
		));
		assert!(matches!(
			check_status(&ready, 8, &[]),
			Err(RipRipError::TestUnitNotReady(None)),
		));

		let read = CommandDescriptorBlock::Twelve([0xBE; 12]);
		assert!(matches!(
			check_status(&read, 2, &sense),
			Err(RipRipError::CdRead),
		));
	}

	#[test]
	fn native_layout() {
		assert_eq!(size_of::<SCSI_PASS_THROUGH_DIRECT>(), 56);
		assert_eq!(offset_of!(Request, sense), 56);
		assert_eq!(size_of::<Request>(), 88);
	}
}
