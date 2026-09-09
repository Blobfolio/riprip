/*!
# Rip Rip Hooray: `libcdio` Wrappers

Resources:
<https://www.t10.org/ftp/t10/document.97/97-117r0.pdf>
<https://github.com/xbmc/libcdio/blob/master/example/cdtext.c>
*/

use crate::{
	Barcode,
	CD_LEADIN,
	CddaDriverExt,
	cdtext::CDText,
	DriveVendorModel,
	macros::log,
	RipRipError,
};
use dactyl::traits::SaturatingFrom;
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
	ffi::{
		CStr,
		CString,
	},
	os::unix::ffi::OsStrExt,
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
pub(crate) struct LibcdioInstance {
	/// # Device.
	dev: Option<CString>,

	/// # CDIO Instance (Pointer).
	ptr: *mut libcdio_sys::CdIo_t,

	/// # CD-Text.
	cdtext: Option<CDText>,
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

impl CddaDriverExt for LibcdioInstance {
	#[expect(unsafe_code, reason = "For FFI.")]
	/// # New!
	fn new<P>(dev: Option<P>) -> Result<Self, RipRipError>
	where P: AsRef<Path> {
		// Make sure the library has been initialized.
		init();

		// Take a look at the desired device.
		let dev =
			if let Some(dev) = dev {
				let dev = dev.as_ref();
				let original: String = dev.to_string_lossy().into_owned();
				if ! dev.exists() {
					return Err(RipRipError::Device(original));
				}
				let dev = CString::new(dev.as_os_str().as_bytes())
					.map_err(|_| RipRipError::Device(original))?;

				log!(@debug "Device path {}.", dev.to_string_lossy());
				Some(dev)
			}
			else { None };

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
	/// # First Track Number.
	fn first_track_num(&self) -> Result<u8, RipRipError> {
		// Safety: this is an FFI call…
		let raw = unsafe {
			libcdio_sys::cdio_get_first_track_num(self.as_ptr())
		};

		if raw == 0 { Err(RipRipError::FirstTrackNum) }
		else { Ok(raw) }
	}

	/// # Leadout.
	fn leadout_lba(&self) -> Result<u32, RipRipError> {
		let idx = u8::try_from(cdio_track_enums_CDIO_CDROM_LEADOUT_TRACK)
			.unwrap_or(170);
		self.track_lba_start(idx)
	}

	#[expect(unsafe_code, reason = "For FFI.")]
	/// # Get the Number of Tracks.
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
	/// # Track Format.
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
	/// # Track LBA Start.
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

	/// # CD-Text.
	///
	/// Return _all_ CD-Text data, if any.
	fn cdtext(&self) -> Option<&CDText> { self.cdtext.as_ref() }

	#[expect(unsafe_code, reason = "For FFI.")]
	/// # Drive Vendor/Model.
	///
	/// Fetch the drive vendor and/or model, if possible.
	fn drive_vendor_model(&self) -> Option<DriveVendorModel> {
		/// # Parse String.
		///
		/// Convert raw vendor/model bytes to a string slice, trimming trailing
		/// null bytes, but otherwise not worrying about the logical sanity of
		/// the value.
		const fn to_str<const N: usize>(raw: &[u8; N]) -> Option<&str> {
			const { assert!(N != 0, "BUG: N cannot be zero."); }

			// The members of `cdio_hwinfo` are one byte longer than the actual
			// data so there should always be a trailing null byte.
			let Ok(cstr) = CStr::from_bytes_until_nul(raw.as_slice()) else {
				std::hint::cold_path();
				return None;
			};

			// UTF-8 validity is less certain. Haha.
			let Ok(out) = cstr.to_str() else { return None; };
			Some(out)
		}

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

			let Some(vendor) = to_str(&vendor_u8) else {
				log!(@trace "Invalid drive vendor {vendor_u8:?}.");
				return None;
			};
			let Some(model) = to_str(&model_u8) else {
				log!(@trace "Invalid drive model {model_u8:?}.");
				return None;
			};
			DriveVendorModel::new(vendor, model).ok()
		}
		else { None }
	}

	#[expect(unsafe_code, reason = "For FFI.")]
	/// # MCN Fallback.
	///
	/// Try pulling MCN via `cdio_get_mcn` in cases where CDText fails.
	fn mcn_subchannel(&self) -> Option<Barcode> {
		// Safety: this is an FFI call…
		let raw = unsafe { libcdio_sys::cdio_get_mcn(self.as_ptr()) };
		if raw.is_null() {
			log!(@trace "Subchannel contains no MCN data.");
			None
		}
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

	#[expect(unsafe_code, reason = "For FFI.")]
	#[expect(non_upper_case_globals, reason = "We don't control these.")]
	#[inline]
	/// # Execute Read Command.
	///
	/// This private method executes the million-argument MMC read command with
	/// values prepared and verified by the caller.
	///
	/// ## Errors.
	///
	/// This will return an error if the read fails, but provides no other
	/// sanity checks.
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
				super::set_bad_sector(lsn);
				Err(RipRipError::CdRead)
			},
		}
	}
}

impl LibcdioInstance {
	/// # As Ptr.
	const fn as_ptr(&self) -> *const libcdio_sys::CdIo_t { self.ptr.cast() }

	/// # As Mut Ptr.
	const fn as_mut_ptr(&self) -> *mut libcdio_sys::CdIo_t { self.ptr }

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
		// Perform a raw read of the CD-Text data, if any.
		// Safety: `libcdio` promises that if there is no data or the read
		// failed, the pointer will be null.
		let ptr = unsafe { libcdio_sys::cdio_get_cdtext_raw(self.as_mut_ptr()) };
		if ptr.is_null() {
			log!(@trace "Disc contains no CDText data.");
			return;
		}

		// Otherwise we need to fetch the length of the allocated array, which
		// is stored in the first two bytes (big endian).
		let p: *const u8 = ptr.cast_const();
		// Safety: the minimum length is therefore two.
		let raw_len = unsafe {
			usize::from((u16::from(*p) << 8) | u16::from(*p.add(1)))
		};

		// The `raw_len` doesn't include itself, but the first two bytes
		// following it are padding, so we need to subtract another two for
		// the relevant total.
		if let Some(len) = raw_len.checked_sub(2) && 2 < len {
			// Safety: see above.
			let packs = unsafe {
				std::slice::from_raw_parts(
					p.add(4), // Skip 2 bytes length, 2 bytes padding.
					len,
				)
			};
			if let Some(cdtext) = CDText::from_bytes(packs) {
				self.cdtext.replace(cdtext);
			}
		}

		// Safety: this is an FFI call…
		unsafe { libcdio_sys::cdio_free(ptr.cast()); }
	}
}



#[expect(unsafe_code, reason = "For FFI.")]
/// # Initialize `libcdio`.
///
/// This is only called once, but to be safe, it is also wrapped in a static to
/// make sure it can never re-initialize.
fn init() {
	LIBCDIO_INIT.call_once(|| {
		log!(@debug "Initializing `libcdio` driver.");
		// Safety: this is an FFI call…
		unsafe { libcdio_sys::cdio_init(); }
	});
}
