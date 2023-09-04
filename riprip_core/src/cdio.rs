/*!
# Rip Rip Hooray: `libcdio` Wrappers
*/

use crate::{
	Barcode,
	CD_DATA_C2_SIZE,
	CD_DATA_SIZE,
	CD_LEADIN,
	CDTextKind,
	DriveVendorModel,
	RipRipError,
};
use libcdio_sys::{
	cdio_hwinfo,
	cdio_track_enums_CDIO_CDROM_LEADOUT_TRACK,
	discmode_t_CDIO_DISC_MODE_CD_DA,
	discmode_t_CDIO_DISC_MODE_CD_MIXED,
	driver_id_t_DRIVER_DEVICE, // The equivalent of "use whatever's best".
	driver_return_code_t_DRIVER_OP_NOT_PERMITTED,
	driver_return_code_t_DRIVER_OP_SUCCESS,
	track_format_t_TRACK_FORMAT_AUDIO,
	track_format_t_TRACK_FORMAT_ERROR,
	track_format_t_TRACK_FORMAT_PSX,
};
use std::{
	ffi::{
		CStr,
		CString,
	},
	ops::Range,
	os::{
		raw::c_char,
		unix::ffi::OsStrExt,
	},
	path::Path,
	sync::Once,
};



/// # Initialization Counter.
static LIBCDIO_INIT: Once = Once::new();



#[derive(Debug)]
/// # CDIO Instance.
///
/// Pretty much all CD-related communications run through a single `libcdio`
/// object. Every interface is unsafe and awkward, so this struct exists to
/// abstract away the noise and handle cleanup.
pub(super) struct LibcdioInstance {
	dev: Option<CString>,
	ptr: *mut libcdio_sys::CdIo_t,
	cdtext: Option<*mut libcdio_sys::cdtext_t>,
}

impl Drop for LibcdioInstance {
	#[allow(unsafe_code)]
	fn drop(&mut self) {
		// Release the C memory!
		if ! self.ptr.is_null() {
			unsafe { libcdio_sys::cdio_destroy(self.as_mut_ptr()); }

			// Use the dev field so Rust won't complain about dead code. Haha.
			self.dev.take();
		}
	}
}

impl LibcdioInstance {
	#[allow(unsafe_code)]
	/// # New!
	///
	/// Initialize a new instance, optionally connecting to a specific device.
	///
	/// ## Errors
	///
	/// This will return an error if initialization fails, or if the provided
	/// device path is obviously wrong.
	pub(super) fn new<P>(dev: Option<P>) -> Result<Self, RipRipError>
	where P: AsRef<Path> {
		// Make sure the library has been initialized.
		init();

		// Take a look at the desired device.
		let dev = {
			if let Some(dev) = dev {
				let dev = dev.as_ref();
				let original: String = dev.to_string_lossy().into_owned();
				if ! dev.exists() {
					return Err(RipRipError::Device(original));
				}
				let dev = CString::new(dev.as_os_str().as_bytes())
					.map_err(|_| RipRipError::Device(original))?;
				Some(dev)
			}
			else { None }
		};

		// Connect to it.
		let ptr = unsafe {
			libcdio_sys::cdio_open(
				dev.as_ref().map_or_else(std::ptr::null, |v| v.as_ptr()),
				driver_id_t_DRIVER_DEVICE,
			)
		};

		// NULL is bad.
		if ptr.is_null() {
			Err(RipRipError::DeviceOpen(dev.map(|v| v.to_string_lossy().into_owned())))
		}
		// Otherwise maybe!
		else {
			let mut out = Self {
				dev,
				ptr,
				cdtext: None,
			};

			// Make sure the disc is present and valid before leaving, and
			// initialize the CDText to have it ready for later queries.
			out._check_disc_mode()?;
			out._init_cdtext();

			// Done!
			Ok(out)
		}
	}

	#[allow(unsafe_code)]
	#[allow(non_upper_case_globals)] // These aren't our globals.
	/// # Check Disc Mode.
	///
	/// This makes sure an audio CD is actually present in the drive.
	///
	/// ## Errors
	///
	/// Returns an error if the disc is missing or unsupported.
	fn _check_disc_mode(&self) -> Result<(), RipRipError> {
		let discmode = unsafe {
			libcdio_sys::cdio_get_discmode(self.as_mut_ptr())
		};
		if matches!(
			discmode,
			discmode_t_CDIO_DISC_MODE_CD_DA | discmode_t_CDIO_DISC_MODE_CD_MIXED
		) {
			Ok(())
		}
		else { Err(RipRipError::DiscMode) }
	}

	#[allow(unsafe_code)]
	/// # Initialize CDText.
	///
	/// This initializes (but does not parse) the CDText information contained
	/// on the disc, if any.
	///
	/// The data on the other end of this pointer gets cleaned up when the
	/// parent instance is destroyed, so it makes sense keeping the two
	/// together.
	fn _init_cdtext(&mut self) {
		let ptr = unsafe {
			libcdio_sys::cdio_get_cdtext(self.as_mut_ptr())
		};
		if ! ptr.is_null() { self.cdtext.replace(ptr); }
	}
}

impl LibcdioInstance {
	/// # As Ptr.
	pub(super) const fn as_ptr(&self) -> *const libcdio_sys::CdIo_t { self.ptr.cast() }

	/// # As Mut Ptr.
	pub(super) const fn as_mut_ptr(&self) -> *mut libcdio_sys::CdIo_t { self.ptr }
}

impl LibcdioInstance {
	#[allow(unsafe_code)]
	/// # First Track Number.
	///
	/// Return the first track number on the disc, almost always but not
	/// necessarily `1`.
	pub(super) fn first_track_num(&self) -> Result<u8, RipRipError> {
		let raw = unsafe {
			libcdio_sys::cdio_get_first_track_num(self.as_ptr())
		};

		if raw == 0 { Err(RipRipError::FirstTrackNum) }
		else { Ok(raw) }
	}

	/// # Leadout.
	///
	/// Return the LBA — including the leading `150` — of the disc leadout.
	pub(super) fn leadout_lba(&self) -> Result<u32, RipRipError> {
		let idx = u8::try_from(cdio_track_enums_CDIO_CDROM_LEADOUT_TRACK)
			.unwrap_or(170);
		self.track_lba_start(idx)
	}

	#[allow(unsafe_code)]
	/// # Get the Number of Tracks.
	///
	/// Return the total number of tracks, or the last track number, however
	/// you want to think of it.
	pub(super) fn num_tracks(&self) -> Result<u8, RipRipError> {
		let raw = unsafe {
			libcdio_sys::cdio_get_num_tracks(self.as_ptr())
		};

		if raw == 0 { Err(RipRipError::NumTracks) }
		else { Ok(raw) }
	}

	#[allow(unsafe_code)]
	#[allow(non_upper_case_globals)] // Not our globals.
	/// # Track Format.
	///
	/// Returns `true` for audio, `false` for data, and an error for anything
	/// else.
	pub(super) fn track_format(&self, idx: u8) -> Result<bool, RipRipError> {
		let kind = unsafe {
			libcdio_sys::cdio_get_track_format(self.as_ptr(), idx)
		};

		match kind {
			track_format_t_TRACK_FORMAT_AUDIO => Ok(true),
			track_format_t_TRACK_FORMAT_PSX |
			track_format_t_TRACK_FORMAT_ERROR => Err(RipRipError::TrackFormat(idx)),
			_ => Ok(false),
		}
	}

	#[allow(unsafe_code)]
	/// # Track LBA Start.
	///
	/// Return the starting LBA — including the leading `150` — for a given
	/// track.
	pub(super) fn track_lba_start(&self, idx: u8) -> Result<u32, RipRipError> {
		if idx == 0 { Err(RipRipError::TrackNumber(0)) }
		else {
			let raw = unsafe {
				libcdio_sys::cdio_get_track_lsn(self.as_ptr(), idx)
			};
			if raw < 0 { Err(RipRipError::TrackLba(idx)) }
			else { Ok(raw.abs_diff(0) + CD_LEADIN) }
		}
	}
}

impl LibcdioInstance {
	#[allow(unsafe_code)]
	/// # CDText Value.
	///
	/// Return the value associated with the CDText field, if any. If the track
	/// number is zero, data associated with the album will be returned.
	pub(super) fn cdtext(&self, idx: u8, kind: CDTextKind) -> Option<String> {
		let ptr = self.cdtext?;
		let raw = unsafe {
			libcdio_sys::cdtext_get_const(
				ptr.cast(),
				kind as u32,
				idx,
			)
		};

		c_char_to_string(raw)
	}

	/*
	#[allow(unsafe_code)]
	/// # Track ISRC.
	///
	/// This method is used as a fallback when the value is not within the
	/// CDText, but is relatively slow.
	pub(super) fn track_isrc(&self, idx: u8) -> Option<String> {
		if self.supports_isrc() {
			let raw = unsafe {
				libcdio_sys::cdio_get_track_isrc(self.as_ptr(), idx)
			};

			let out = c_char_to_string(raw.cast());
			unsafe { libcdio_sys::cdio_free(raw.cast()); }
			out
		}
		else { None }
	}
	*/

	/// # MCN.
	///
	/// Return the disc's associated UPC/EAN, if present. This will try CDText
	/// first since that data is already loaded, and fall back to the direct
	/// `cdio_get_mcn` request if that doesn't work.
	pub(super) fn mcn(&self) -> Option<Barcode> {
		// It probably isn't in CDText, but we already have it, so might as
		// well check there first.
		self.cdtext(0, CDTextKind::Barcode)
			.and_then(|v| Barcode::try_from(v.as_bytes()).ok())
			// Otherwise try pulling it directly.
			.or_else(|| self._mcn())
	}

	#[allow(unsafe_code)]
	/// # MCN Fallback.
	///
	/// Try pulling MCN via `cdio_get_mcn` in cases where CDText fails.
	fn _mcn(&self) -> Option<Barcode> {
		let raw = unsafe {
			libcdio_sys::cdio_get_mcn(self.as_ptr())
		};
		if raw.is_null() { None }
		else {
			let mcn = unsafe { CStr::from_ptr(raw) }
				.to_str()
				.ok()
				.and_then(|v| Barcode::try_from(v.as_bytes()).ok());
			unsafe { libcdio_sys::cdio_free(raw.cast()); }
			mcn
		}
	}
}

impl LibcdioInstance {
	#[allow(unsafe_code, clippy::cast_sign_loss)]
	/// # Drive Vendor/Model.
	///
	/// Fetch the drive vendor and/or model, if possible.
	pub(super) fn drive_vendor_model(&self) -> Option<DriveVendorModel> {
		let mut raw = cdio_hwinfo {
			psz_vendor: [0; 9],
			psz_model: [0; 17],
			psz_revision: [0; 5],
		};

		// The return code is a bool, true for good, instead of the usual
		// 0 for good.
		if 1 == unsafe { libcdio_sys::cdio_get_hwinfo(self.as_ptr(), &mut raw) } {
			// Rather than deal with the uncertainty of pointers, let's recast
			// the signs since we have everything right here.
			let vendor_u8 = raw.psz_vendor.map(|b| b as u8);
			let model_u8 = raw.psz_model.map(|b| b as u8);

			// Vendor might be empty.
			let vendor =
				if vendor_u8[0] == 0 { "" }
				else {
					CStr::from_bytes_until_nul(vendor_u8.as_slice())
					.ok()
					.and_then(|v| v.to_str().ok())?
				};

			// But model is required.
			let model =
				if model_u8[0] == 0 { None }
				else {
					CStr::from_bytes_until_nul(model_u8.as_slice())
					.ok()
					.and_then(|v| v.to_str().ok())
				}?;

			DriveVendorModel::new(vendor, model).ok()
		}
		else { None }
	}
}

impl LibcdioInstance {
	#[allow(unsafe_code)]
	#[allow(non_upper_case_globals)] // Not our globals.
	/// # Read Raw.
	///
	/// This attempts to read a single audio sector — and maybe C2 data — to
	/// the provided buffer. The buffer size is used to determine which mode is
	/// being requested.
	///
	/// ## Errors
	///
	/// This will return an error if the buffer is insufficient, the read
	/// operation is unsupported, or the disc is too messed up to be read.
	pub(super) fn read_cd(
		&self,
		buf: &mut [u8],
		lsn: i32,
	) -> Result<(), RipRipError> {
		// The buffer and block size are equivalent for our purposes.
		let block_size = u16::try_from(buf.len())
			.map_err(|_| RipRipError::CdReadBuffer)?;

		// We can infer whether or not C2 is desired based on the block size,
		// and at the same time rule out wacky sizes.
		let c2_too = match u32::from(block_size) {
			CD_DATA_C2_SIZE => 1,
			CD_DATA_SIZE => 0,
			_ => return Err(RipRipError::CdReadBuffer),
		};

		// Reset the buffer before beginning.
		for v in &mut *buf { *v = 0; }

		// We don't need to worry about reading negative ranges.
		if lsn < 0 { return Ok(()); }

		// We should, however, read anything else!
		let res = unsafe {
			libcdio_sys::mmc_read_cd(
				self.as_ptr(),
				buf.as_mut_ptr().cast(),
				lsn,
				1,      // Sector type: CDDA.
				0,      // No random data manipulation thank you kindly.
				0,      // No header syncing.
				0,      // No headers.
				1,      // YES audio block!
				0,      // No EDC.
				c2_too,
				0,      // No subchannel.
				block_size,
				1,      // One block at a time.
			)
		};
		match res {
			driver_return_code_t_DRIVER_OP_NOT_PERMITTED => Err(RipRipError::CdReadUnsupported),
			driver_return_code_t_DRIVER_OP_SUCCESS => Ok(()),
			_ => Err(RipRipError::CdRead(lsn)),
		}
	}

	#[allow(unsafe_code)]
	#[allow(clippy::cast_possible_truncation)]
	/// # Cache Bust.
	///
	/// In lieu of any universal I/O command to clear the drive cache, we can
	/// do the next best thing: fill its buffers with other stuff!
	///
	/// This will read 1800 sectors — preferrably outside the target track's
	/// range — to ensure that when we ask for the target track's data, the
	/// drive will have to _actually read it_.
	///
	/// That's the theory anyway.
	///
	/// Thankfully we're never requesting the same sector back-to-back, so only
	/// need to do this at the start of each rip pass.
	pub(super) fn bust_cache(
		&self,
		blacklist: Range<i32>,
		leadout: i32,
	) {
		// Slightly more than 4MiB; should be enough for any drive.
		let mut buf = vec![0; 1800 * CD_DATA_SIZE as usize];

		// Where can we read from without getting in the way?
		let lsn =
			// Read from the start of the disc.
			if 1800 < blacklist.start { 0 }
			// Read after the current track.
			else if blacklist.end + 1800 < leadout { blacklist.end }
			// Read the last 1800 sectors on the disc.
			else { leadout - 1800 };

		// We should, however, read anything else!
		let _res = unsafe {
			libcdio_sys::mmc_read_cd(
				self.as_ptr(),
				buf.as_mut_ptr().cast(),
				lsn,
				1,      // Sector type: CDDA.
				0,      // No random data manipulation thank you kindly.
				0,      // No header syncing.
				0,      // No headers.
				1,      // YES audio block!
				0,      // No EDC.
				0,      // No C2.
				0,      // No subchannel.
				CD_DATA_SIZE as u16,
				1800,   // Number of blocks to read.
			)
		};
	}
}



#[allow(unsafe_code)]
/// # Initialize `libcdio`.
///
/// This is only called once, but to be safe, it is also wrapped in a static to
/// make sure it can never re-initialize.
fn init() {
	LIBCDIO_INIT.call_once(|| unsafe { libcdio_sys::cdio_init(); });
}

#[allow(unsafe_code)]
/// # Pointer to String.
///
/// Convert C-string pointers to a string, unless they're null.
fn c_char_to_string(ptr: *const c_char) -> Option<String> {
	if ptr.is_null() { None }
	else {
		unsafe { CStr::from_ptr(ptr) }
			.to_str()
			.ok()
			.map(|s| s.trim().to_owned())
			.filter(|s| ! s.is_empty())
	}
}
