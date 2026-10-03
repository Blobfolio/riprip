/*!
# Rip Rip Hooray: `libsg` Wrappers

Somewhat useful documentation:
<https://www.kernel.org/doc/html/latest/scsi/scsi-generic.html>
*/

use crate::{
	CddaDriverNewExt,
	RipRipError,
	macros::log,
};
use dactyl::NiceU64;
use rustix::{
	ffi::c_void,
	ioctl::Updater,
};
use std::{
	fmt,
	fs::{
		OpenOptions,
		File,
	},
	os::unix::fs::MetadataExt,
	path::PathBuf,
};
use super::{
	mmc::{
		CommandDescriptorBlock,
		MmcDriverExt,
		TransportExt,
	},
	open_err,
	path_is_block_char_device,
};

/// # Default Device Path.
///
/// SG IO being kernel-based means we can assume certain conventions that
/// other drivers can't. Haha.
const DEFAULT_PATH: &str = "/dev/sr0";

#[repr(C)]
/// # SG IO.
///
/// This ghastly struct recreates the the Linux kernel's `sg_io_hdr_t` type,
/// used for SCSI Generic (SG) `ioctl` data.
///
/// See also: <https://tldp.org/HOWTO/SCSI-Generic-HOWTO/sg_io_hdr_t.html>
struct SgIoHdr {
	/// # Interface ID.
	///
	/// This is always `b"S"`.
	interface_id: i32,

	/// # Transfer Direction.
	///
	/// This is normally `SG_DXFER_FROM_DEV` (-3), except in the case of a TEST
	/// UNIT READY request, in which case it's `SG_DXFER_NONE` (-1).
	dxfer_direction: i32,

	/// # Command Descriptor Block Size.
	///
	/// This is the length of the CDB. For our purposes, it'll always be six,
	/// ten, or twelve.
	cmd_len: u8,

	/// # Max Sense Length.
	///
	/// The length of `sbp`.
	///
	/// TODO: can this be zero?
	mx_sb_len: u8,

	/// # UNUSED: Scatter Gather Count.
	///
	/// This is always zero for us.
	iovec_count: u16,

	/// # Transfer Length.
	///
	/// The length of `dxferp`.
	dxfer_len: u32,

	/// # Data Buffer.
	dxferp: *mut c_void,

	/// # Command Block Descriptor.
	cmdp: *mut u8,

	/// # Sense Block.
	sbp: *mut u8,

	/// # Timeout (ms).
	timeout: u32,

	/// # UNUSED: Flags.
	///
	/// This could be used to enable e.g. direct IO, but we don't need it.
	flags: u32,

	/// # UNUSED: Pack ID.
	///
	/// This is mainly relevant to queued requests, which we aren't doing.
	pack_id: i32,

	/// # UNUSED: User Pointer.
	///
	/// This allows a user (us) to associate arbitrary data with the request.
	/// We don't have any.
	usr_ptr: *mut c_void,

	/// # UNUSED: Status Code.
	///
	/// This allegedly holds the official SCSI status code, with or without
	/// a few extra bits supplied by the device. Not very useful for our
	/// purposes.
	status: u8,

	/// # Masked Status.
	///
	/// This is a normalized representation of `status`, i.e. what we should
	/// be checking. The `ScsiStatus` enum maps to the possible values.
	masked_status: u8,

	/// # UNUSED: Message Status.
	///
	/// According to the docs, this isn't really used anywhere. Haha.
	msg_status: u8,

	/// # Actual Sense Length.
	///
	/// The amount of sense data written. This will never exceed `mx_sb_len`,
	/// and will often be zero.
	sb_len_wr: u8,

	/// # UNUSED: Host Status.
	///
	/// This is used to convey I/O status in a manner more similar to what the
	/// kernel itself sees.
	host_status: u16,

	/// # UNUSED: Driver Status.
	///
	/// This holds a driver-specific status, mainly useful in contexts where
	/// a single driver controls multiple devices. We aren't currently doing
	/// anything with it, but might some day?
	driver_status: u16,

	/// # Residual Data.
	///
	/// The number of bytes _not_ written to the buffer even though we asked
	/// for them. This should almost always be zero.
	resid: i32,

	/// # UNUSED: Duration (ms).
	///
	/// The roundtrip command time.
	duration: u32,

	/// # UNUSED: Information.
	///
	/// As the name suggests, this acts like a sort of informational catch-all.
	///
	/// The first bit indicates whether or not there are any non-zero sense,
	/// host, and/or driver status.
	///
	/// This can also be used to indicate whether or not direct or indirect IO
	/// happened, but we aren't messing with that.
	info: u32,
}

/// # SG IO Instance.
///
/// When the SG driver is enabled, an instance of this struct is used to
/// facilitate all host/device communications.
///
/// At present it simply holds an open file reference to the device's
/// `/dev/sgN` path.
pub(crate) struct SgIoInstance {
	/// # Device "File".
	device: File,
}

impl TransportExt for SgIoInstance {
	#[expect(unsafe_code, reason = "For FFI.")]
	/// # Submit.
	fn submit(
		&self,
		cdb: &CommandDescriptorBlock,
		buf: &mut [u8],
		ctx: &'static str,
	) -> Result<usize, RipRipError> {
		// Sense size.
		const MX_SB_LEN: u8 = 32;

		// Our buffers always fit `u16` so this should never fail.
		let Ok(dxfer_len) = u32::try_from(buf.len()) else {
			std::hint::cold_path();
			log!(@trace [ctx, buf.len()] "Bug: transfer length exceeds `u32`.");
			return Err(RipRipError::Bug("Transfer length exceeds `u32`."));
		};

		// SG_DXFER_NONE if no data, otherwise SG_DXFER_FROM_DEV.
		let dxfer_direction: i32 = if buf.is_empty() { -1 } else { -3 };

		// Buffer for the sense data.
		let mut sense = [0_u8; MX_SB_LEN as usize];

		// Put it all together. Ug. Haha.
		let mut hdr = SgIoHdr {
			interface_id: i32::from(b'S'),
			dxfer_direction,
			cmd_len: cdb.len(),
			mx_sb_len: MX_SB_LEN,
			iovec_count: 0,
			dxfer_len,
			dxferp: buf.as_mut_ptr().cast(),
			cmdp: cdb.as_slice().as_ptr().cast_mut(),
			sbp: sense.as_mut_ptr(),
			timeout: 10_000,
			flags: 0,
			pack_id: 0,
			usr_ptr: std::ptr::null_mut(),
			status: 0,
			masked_status: 0,
			msg_status: 0,
			sb_len_wr: 0,
			host_status: 0,
			driver_status: 0,
			resid: 0,
			duration: 0,
			info: 0,
		};

		// Safety: this is an FFI call…
		unsafe {
			rustix::ioctl::ioctl(
				&self.device,
				Updater::<0x2285, SgIoHdr>::new(&mut hdr)
			)
		}.map_err(|e| {
			log!(@trace [ctx, cdb, buf] "Command block write failed: {e}.");
			RipRipError::Internal(ctx, e.to_string())
		})?;

		// The actual transfer length will usually be the full buffer length.
		let mut transferred = buf.len();

		// In the unlikely even there's residual data, log it and adjust the
		// total accordingly.
		let residual = usize::try_from(hdr.resid).unwrap_or(0);
		if residual != 0 {
			log!(
				@trace [ctx, buf.len(), residual]
				"Drive reported {residual} residual bytes of data.",
				residual=NiceU64::from(residual),
			);
			transferred = transferred.saturating_sub(residual);
		}

		// How'd we do?
		match ScsiStatus::from_u8(hdr.masked_status) {
			// Yay!
			Some(ScsiStatus::Good | ScsiStatus::ConditionGood | ScsiStatus::IntermediateGood | ScsiStatus::IntermediateCGood) => Ok(transferred),

			// Device might not be ready. Only applies to TEST UNIT READY.
			Some(ScsiStatus::CheckCondition) if cdb.is_test_unit_ready() =>
				// Fixed format sense data.
				if 13 < hdr.sb_len_wr && matches!(sense[0], 0x70 | 0x71) {
					Err(RipRipError::TestUnitNotReady(Some([
						sense[2] & 0x0F,
						sense[12],
						sense[13],
					])))
				}
				// Descriptor format sense data.
				else if 3 < hdr.sb_len_wr && matches!(sense[0], 0x72 | 0x73) {
					Err(RipRipError::TestUnitNotReady(Some([
						sense[1] & 0x0F,
						sense[2],
						sense[3],
					])))
				}
				// Unknown…
				else { Err(RipRipError::TestUnitNotReady(None)) },

			// Device is definitely not ready. Only applies to TEST UNIT
			Some(ScsiStatus::Busy) if cdb.is_test_unit_ready() => Err(RipRipError::TestUnitNotReady(None)),

			// It's not entirely clear what the remaining statuses are for or
			// if they'd ever pop up, but we know their names so can write up
			// a report before returning an error.
			Some(status) => {
				log!(
					@trace [ctx, cdb, buf, transferred, sense]
					"Command status ({status} / 0x{:02X}) did not pass.",
					status as u8,
				);
				Err(RipRipError::CdRead)
			},

			// If the status _didn't_ map to our official list, it's anybody's
			// guess what happened. Haha.
			None => {
				log!(
					@trace [ctx, cdb, buf, transferred, sense]
					"Command status (0x{:02X}) did not pass.",
					hdr.masked_status,
				);
				Err(RipRipError::CdRead)
			},
		}
	}
}

impl CddaDriverNewExt for SgIoInstance {
	/// # New Instance.
	fn new(dev: Option<PathBuf>) -> Result<Self, RipRipError> {
		log!(@debug "Initializing `SG IO` driver.");

		let dev = match dev {
			// If provided, the path will already be canonical.
			Some(v) => v,
			// The fallback may or may not exist.
			None => std::fs::canonicalize(DEFAULT_PATH)
				.map_err(|_| open_err(None))?,
		};

		// Double-check the path maps to a block/char device, and convert it
		// to a more authoritative sysfs root that, hopefully, contains the
		// associated SG name.
		let meta = path_is_block_char_device(&dev)?;
		let dev_num = meta.rdev();
		let syspath = format!(
			"/sys/dev/block/{}:{}/device/scsi_generic/",
			rustix::fs::major(dev_num),
			rustix::fs::minor(dev_num),
		);
		let Ok(syspath) = std::fs::canonicalize(&syspath) else {
			log!(@trace [dev, syspath] "Unable to resolve syspath for device.");
			return Err(open_err(Some(&dev)));
		};

		// We have to crawl the directory to find the SG.
		let sgpath = std::fs::read_dir(&syspath).ok()
			.and_then(|mut rd| rd.find_map(|e|
				if
					let Ok(e) = e &&
					let Some(name) = e.file_name().to_str() &&
					let [ b's', b'g', rest @ .. ] = name.as_bytes() &&
					! rest.is_empty() &&
					rest.iter().all(u8::is_ascii_digit)
				{
					std::fs::canonicalize(format!("/dev/{name}")).ok()
				}
				else { None }
			))
			.ok_or_else(|| {
				log!(@trace [dev, syspath] "Unable to resolve SG path for device.");
				open_err(Some(&dev))
			})?;

		// Now we just need to open it like a "file"!
		let device = OpenOptions::new()
			.read(true)
			.write(false)
			.open(&sgpath)
			.map_err(|e| {
				log!(@trace [dev, syspath, sgpath] "Failed to open device: {e}.");
				open_err(Some(&dev))
			})?;

		log!(
			@trace [dev]
			"Found SG device path {}.",
			sgpath.display(),
		);

		let out = Self { device };
		out.check_disc_mode__()?;
		out.check_c2__()?;
		Ok(out)
	}
}



/// # Helper: SCSI Status Codes.
macro_rules! scsi_status {
	( $( $k:ident $v:literal $str:literal, )+ ) => (
		#[repr(u8)]
		#[derive(Debug, Clone, Copy, Eq, PartialEq)]
		/// # (Normalized) SCSI Status Codes.
		enum ScsiStatus {
			$(
				#[doc = concat!("# ", $str, ".")]
				$k = $v,
			)+
		}

		impl fmt::Display for ScsiStatus {
			fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.write_str(match *self {
					$( Self::$k => $str, )+
				})
			}
		}

		impl ScsiStatus {
			#[must_use]
			/// # From `u8`.
			const fn from_u8(raw: u8) -> Option<Self> {
				match raw {
					$( $v => Some(Self::$k), )+
					_ => None,
				}
			}
		}
	);
}

scsi_status! {
	Good                0x00 "`GOOD`",
	CheckCondition      0x01 "`CHECK_CONDITION`",
	ConditionGood       0x02 "`CONDITION_GOOD`",
	Busy                0x04 "`BUSY`",
	IntermediateGood    0x08 "`INTERMEDIATE_GOOD`",
	IntermediateCGood   0x0a "`INTERMEDIATE_C_GOOD`",
	ReservationConflict 0x0c "`RESERVATION_CONFLICT`",
	CommandTerminated   0x11 "`COMMAND_TERMINATED`",
	QueueFull           0x14 "`QUEUE_FULL`",
}
