/*!
# Rip Rip Hooray: `libcdio` Wrappers

Somewhat useful documentation:
<https://www.t10.org/ftp/t10/document.97/97-117r0.pdf>
*/

use crate::{
	Barcode,
	CD_DATA_C2_SIZE,
	CD_DATA_SIZE,
	CD_DATA_SUBCHANNEL_SIZE,
	CD_LEADIN,
	CDTextKind,
	DriveVendorModel,
	KillSwitch,
	RipRipError,
};
use crate::Cdda;
use dactyl::{
	NoHash,
	traits::SaturatingFrom,
};
use libcdio_sys::{
	cdio_hwinfo,
	cdio_track_enums_CDIO_CDROM_LEADOUT_TRACK,
	discmode_t_CDIO_DISC_MODE_CD_DA,
	discmode_t_CDIO_DISC_MODE_CD_MIXED,
	driver_id_t_DRIVER_DEVICE, // The equivalent of "use whatever's best".
	driver_return_code_t_DRIVER_OP_NOT_PERMITTED,
	driver_return_code_t_DRIVER_OP_SUCCESS,
	driver_return_code_t_DRIVER_OP_UNSUPPORTED,
	track_format_t_TRACK_FORMAT_AUDIO,
	track_format_t_TRACK_FORMAT_ERROR,
	track_format_t_TRACK_FORMAT_PSX,
};
use std::{
	cell::RefCell,
	collections::HashSet,
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
	time::{
		Duration,
		Instant,
	},
};



/// # Cache Bust Timeout.
const CACHE_BUST_TIMEOUT: Duration = Duration::from_secs(45);

/// # Initialization Counter.
static LIBCDIO_INIT: Once = Once::new();

thread_local! {
	/// # Sector Shitlist.
	///
	/// Keep track of sectors that trigger hard read errors so we don't
	/// accidentally try them in a cache-bust situation.
	static SHITLIST: RefCell<HashSet<i32, NoHash>> = RefCell::new(HashSet::with_hasher(NoHash::default()));
}



#[derive(Debug)]
/// # CDIO Instance.
///
/// Pretty much all CD-related communications run through a single `libcdio`
/// object. Every interface is unsafe and awkward, so this struct exists to
/// abstract away the noise and handle cleanup.
pub(super) struct LibcdioInstance {
	/// # Device.
	dev: Option<CString>,

	/// # CDIO Instance (Pointer).
	ptr: *mut libcdio_sys::CdIo_t,

	/// # CD-Text (Pointer).
	cdtext: Option<*mut libcdio_sys::cdtext_t>,
}

impl Drop for LibcdioInstance {
	#[expect(unsafe_code, reason = "For FFI.")]
	fn drop(&mut self) {
		// Release the C memory!
		if ! self.ptr.is_null() {
			// Safety: this is an FFI call…
			unsafe { libcdio_sys::cdio_destroy(self.as_mut_ptr()); }

			// Use the dev field so Rust won't complain about dead code. Haha.
			self.dev.take();
		}
	}
}

impl LibcdioInstance {
	#[expect(unsafe_code, reason = "For FFI.")]
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
		// Safety: this is an FFI call…
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
			out.check_disc_mode__()?;
			out.init_cdtext__();

			// Done!
			Ok(out)
		}
	}

	#[expect(unsafe_code, reason = "For FFI.")]
	#[expect(non_upper_case_globals, reason = "We don't control these.")]
	/// # Check Disc Mode.
	///
	/// This makes sure an audio CD is actually present in the drive.
	///
	/// ## Errors
	///
	/// Returns an error if the disc is missing or unsupported.
	fn check_disc_mode__(&self) -> Result<(), RipRipError> {
		// Safety: this is an FFI call…
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

	#[expect(unsafe_code, reason = "For FFI.")]
	/// # Initialize CDText.
	///
	/// This initializes (but does not parse) the CDText information contained
	/// on the disc, if any.
	///
	/// The data on the other end of this pointer gets cleaned up when the
	/// parent instance is destroyed, so it makes sense keeping the two
	/// together.
	fn init_cdtext__(&mut self) {
		// Safety: this is an FFI call…
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
	/*
	#[expect(unsafe_code, reason = "For FFI.")]
	/// # Track ISRC.
	///
	/// This method is used as a fallback when the value is not within the
	/// CDText, but is relatively slow.
	pub(super) fn track_isrc(&self, idx: u8) -> Option<String> {
		if self.supports_isrc() {
			// Safety: this is an FFI call…
			let raw = unsafe {
				libcdio_sys::cdio_get_track_isrc(self.as_ptr(), idx)
			};

			let out = c_char_to_string(raw.cast());
			// Safety: this is an FFI call…
			unsafe { libcdio_sys::cdio_free(raw.cast()); }
			out
		}
		else { None }
	}
	*/

	#[expect(unsafe_code, reason = "For FFI.")]
	/// # MCN Fallback.
	///
	/// Try pulling MCN via `cdio_get_mcn` in cases where CDText fails.
	fn mcn__(&self) -> Option<Barcode> {
		// Safety: this is an FFI call…
		let raw = unsafe {
			libcdio_sys::cdio_get_mcn(self.as_ptr())
		};
		if raw.is_null() { None }
		else {
			// Safety: this is an FFI call…
			let mcn = unsafe { CStr::from_ptr(raw) }
				.to_str()
				.ok()
				.and_then(|v| Barcode::try_from(v.as_bytes()).ok());
			// Safety: this is an FFI call…
			unsafe { libcdio_sys::cdio_free(raw.cast()); }
			mcn
		}
	}
}

impl Cdda for LibcdioInstance {
	#[expect(unsafe_code, reason = "For FFI.")]
	fn first_track_num(&self) -> Result<u8, RipRipError> {
		// Safety: this is an FFI call…
		let raw = unsafe {
			libcdio_sys::cdio_get_first_track_num(self.as_ptr())
		};

		if raw == 0 { Err(RipRipError::FirstTrackNum) }
		else { Ok(raw) }
	}

	#[expect(unsafe_code, reason = "For FFI.")]
	fn leadout_lba(&self) -> Result<u32, RipRipError> {
		let idx = u8::try_from(cdio_track_enums_CDIO_CDROM_LEADOUT_TRACK)
			.unwrap_or(170);
		self.track_lba_start(idx)
	}

	#[expect(unsafe_code, reason = "For FFI.")]
	fn num_tracks(&self) -> Result<u8, RipRipError> {
		// Safety: this is an FFI call…
		let raw = unsafe {
			libcdio_sys::cdio_get_num_tracks(self.as_ptr())
		};

		if raw == 0 { Err(RipRipError::NumTracks) }
		else { Ok(raw) }
	}

	#[expect(unsafe_code, reason = "For FFI.")]
	#[expect(non_upper_case_globals, reason = "We don't control these.")]
	fn track_format(&self, idx: u8) -> Result<bool, RipRipError> {
		// Safety: this is an FFI call…
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

	#[expect(unsafe_code, reason = "For FFI.")]
	fn track_lba_start(&self, idx: u8) -> Result<u32, RipRipError> {
		if idx == 0 { Err(RipRipError::TrackNumber(0)) }
		else {
			// Safety: this is an FFI call…
			let raw = unsafe {
				libcdio_sys::cdio_get_track_lsn(self.as_ptr(), idx)
			};
			if raw < 0 { Err(RipRipError::TrackLba(idx)) }
			else { Ok(raw.abs_diff(0) + u32::from(CD_LEADIN)) }
		}
	}

	#[expect(unsafe_code, reason = "For FFI.")]
	fn cdtext(&self, idx: u8, kind: CDTextKind) -> Option<String> {
		let ptr = self.cdtext?;
		// Safety: this is an FFI call…
		let raw = unsafe {
			libcdio_sys::cdtext_get_const(
				ptr.cast(),
				kind as u32,
				idx,
			)
		};

		c_char_to_string(raw)
	}

	#[expect(unsafe_code, reason = "For FFI.")]
	fn mcn(&self) -> Option<Barcode> {
		// It probably isn't in CDText, but we already have it, so might as
		// well check there first.
		self.cdtext(0, CDTextKind::Barcode)
			.and_then(|v| Barcode::try_from(v.as_bytes()).ok())
			// Otherwise try pulling it directly.
			.or_else(|| self.mcn__())
	}

	#[expect(unsafe_code, reason = "For FFI.")]
	fn drive_vendor_model(&self) -> Option<DriveVendorModel> {
		let mut raw = cdio_hwinfo {
			psz_vendor: [0; 9],
			psz_model: [0; 17],
			psz_revision: [0; 5],
		};

		// The return code is a bool, true for good, instead of the usual
		// 0 FFI normally kicks back.
		// Safety: this is an FFI call…
		if unsafe { libcdio_sys::cdio_get_hwinfo(self.as_ptr(), &raw mut raw) } {
			// Rather than deal with the uncertainty of pointers, let's recast
			// the signs since we have everything right here.
			let vendor_u8 = raw.psz_vendor.map(u8::saturating_from);
			let model_u8 = raw.psz_model.map(u8::saturating_from);

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

	fn is_sector_bad(&self, lsn: i32) -> bool {
		SHITLIST.with_borrow(|q| q.contains(&lsn))
	}

	#[expect(unsafe_code, reason = "For FFI.")]
	#[expect(non_upper_case_globals, reason = "We don't control these.")]
	#[inline]
	fn read_cd(
		&self,
		buf: &mut [u8],
		lsn: i32,
		c2: bool,
		sub: u8,
		block_size: u16,
	) -> Result<(), RipRipError> {
		// Safety: this is an FFI call…
		let res = unsafe {
			libcdio_sys::mmc_read_cd(
				self.as_ptr(),
				buf.as_mut_ptr().cast(),
				lsn,
				1,            // Sector type: CDDA.
				false,        // No random data manipulation thank you kindly.
				false,        // No header syncing.
				0,            // No headers.
				true,         // YES audio block!
				false,        // No EDC.
				u8::from(c2), // C2 or no C2?
				sub,          // Subchannel? What kind?
				block_size,   // Block size (varies by data requested).
				1,            // Always read one block at a time.
			)
		};

		match res {
			driver_return_code_t_DRIVER_OP_NOT_PERMITTED => Err(RipRipError::CdReadNotPermitted),
			driver_return_code_t_DRIVER_OP_SUCCESS => Ok(()),
			driver_return_code_t_DRIVER_OP_UNSUPPORTED => Err(RipRipError::CdReadUnsupported),
			_ => {
				SHITLIST.with(|q| q.borrow_mut().insert(lsn));
				Err(RipRipError::CdRead)
			},
		}
	}
}



#[expect(unsafe_code, reason = "For FFI.")]
/// # Pointer to String.
///
/// Convert C-string pointers to a string, unless they're null.
fn c_char_to_string(ptr: *const c_char) -> Option<String> {
	if ptr.is_null() { None }
	else {
		// Safety: this is an FFI call…
		unsafe { CStr::from_ptr(ptr) }
			.to_str()
			.ok()
			.map(|s| s.trim().to_owned())
			.filter(|s| ! s.is_empty())
	}
}

#[expect(unsafe_code, reason = "For FFI.")]
/// # Initialize `libcdio`.
///
/// This is only called once, but to be safe, it is also wrapped in a static to
/// make sure it can never re-initialize.
fn init() {
	// Safety: this is an FFI call…
	LIBCDIO_INIT.call_once(|| unsafe { libcdio_sys::cdio_init(); });
}
