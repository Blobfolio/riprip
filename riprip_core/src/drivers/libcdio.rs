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
	CddaDriverNewExt,
	DriveVendorModel,
	Isrc,
	macros::log,
	RipRipError,
};
use libcdio_sys::{
	cdio_drive_cap_read_t_CDIO_DRIVE_CAP_READ_ISRC,
	cdio_drive_cap_read_t_CDIO_DRIVE_CAP_READ_MCN,
	cdio_hwinfo,
	cdio_subchannel_CDIO_SUBCHANNEL_MEDIA_CATALOG,
	cdio_subchannel_CDIO_SUBCHANNEL_TRACK_ISRC,
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
	ffi::CString,
	os::unix::ffi::OsStrExt,
	path::Path,
	sync::Once,
};
use super::{
	SUBCHANNEL_REREADS,
	SUBCHANNEL_REREAD_DELAY,
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

	/// # Flags.
	flags: u8,
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

impl CddaDriverNewExt for LibcdioInstance {
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
			let mut out = Self { dev, ptr, flags: 0 };

			// Make sure the disc is present and valid before leaving, and
			// initialize the CD-Text to have it ready for later queries.
			out.check_disc_mode__()?;

			// Check capabilities.
			out.check_capabilities__();

			// Done!
			Ok(out)
		}
	}
}

impl CddaDriverExt for LibcdioInstance {
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

	#[expect(unsafe_code, reason = "For FFI.")]
	/// # CD-Text (Raw).
	///
	/// Read and return the raw CD-Text data, if any.
	fn cdtext(&self) -> Option<Vec<u8>> {
		// Perform a raw read of the CD-Text data, if any.
		// Safety: `libcdio` promises that if there is no data or the read
		// failed, the pointer will be null.
		let ptr = unsafe { libcdio_sys::cdio_get_cdtext_raw(self.as_mut_ptr()) };
		if ptr.is_null() {
			log!(@trace "Disc contains no CD-Text data.");
			return None;
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
		let out =
			if let Some(len) = raw_len.checked_sub(2) && 2 < len {
				// Safety: see above.
				unsafe {
					std::slice::from_raw_parts(
						p.add(4), // Skip 2 bytes length, 2 bytes padding.
						len,
					)
				}.to_vec()
			}
			else { Vec::new() };

		// Safety: this is an FFI call…
		unsafe { libcdio_sys::cdio_free(ptr.cast()); }

		// Done!
		if out.is_empty() { None }
		else { Some(out) }
	}

	#[expect(unsafe_code, reason = "For FFI.")]
	/// # Drive Vendor/Model.
	///
	/// Fetch the drive vendor and/or model, if possible.
	fn drive_vendor_model(&self) -> Option<DriveVendorModel> {
		/// # Rustify Cstr Arrays.
		///
		/// The `libcdio` member arrays are `i8` for reasons…
		const fn normalize<const N: usize>(src: [i8; N]) -> [u8; N] {
			let mut out = [0_u8; N];
			let mut i = 0;
			while i < out.len() {
				if src[i] <= 0 { out[i] = 0; }
				else { out[i] = src[i].cast_unsigned(); }
				i += 1;
			}
			out
		}

		let mut raw = cdio_hwinfo {
			psz_vendor:   [0; DriveVendorModel::VENDOR_LEN + 1],
			psz_model:    [0; DriveVendorModel::MODEL_LEN + 1],
			psz_revision: [0; DriveVendorModel::REVISION_LEN + 1],
		};

		// The return code is a bool, true for good, instead of the usual
		// 0 FFI normally kicks back.
		// Safety: this is an FFI call…
		if unsafe { libcdio_sys::cdio_get_hwinfo(self.as_ptr(), &raw mut raw) } {
			// Recast as normal-ass bytes.
			let vendor = normalize(raw.psz_vendor);
			let model = normalize(raw.psz_model);
			let revision = normalize(raw.psz_revision);

			DriveVendorModel::new(&vendor, &model, &revision).ok()
		}
		else { None }
	}

	/// # ISRC From Subchannel.
	fn isrc_subchannel(&self, idx: u8) -> Option<Isrc> {
		/// # ISRC From Subchannel.
		fn fetch(cdda: &LibcdioInstance, idx: u8) -> Option<Isrc> {
			// Give it a few tries because subchannel reads suck. Haha.
			let mut raw = None;
			for i in 0..SUBCHANNEL_REREADS {
				if let Some(read) = read_isrc_mcn_subchannel::<cdio_subchannel_CDIO_SUBCHANNEL_TRACK_ISRC>(
					cdda,
					idx,
				) {
					raw.replace(read);
					break;
				}
				// Wait before re-attempting.
				else if i + 1 < SUBCHANNEL_REREADS {
					std::thread::sleep(SUBCHANNEL_REREAD_DELAY);
				}
			}

			// Try the whole buffer first.
			let raw = raw?;
			Isrc::try_from(raw.as_slice())
				// Fall back to the punctuation-free length.
				.or_else(|_| Isrc::try_from(&raw[..12]))
				.ok()
		}

		// Unsupported?
		if 0 == self.flags & Self::FLAG_SUPPORTS_SUBCHANNEL_ISRC {
			std::hint::cold_path();
			None
		}
		else { fetch(self, idx) }
	}

	/// # MCN Fallback.
	///
	/// Try pulling MCN via `cdio_get_mcn` in cases where CD-Text fails.
	fn mcn_subchannel(&self) -> Option<Barcode> {
		/// # ISRC From Subchannel.
		fn fetch(cdda: &LibcdioInstance, idx: u8) -> Option<Barcode> {
			// Give it a few tries because subchannel reads suck. Haha.
			let mut raw = None;
			for i in 0..SUBCHANNEL_REREADS {
				if let Some(read) = read_isrc_mcn_subchannel::<cdio_subchannel_CDIO_SUBCHANNEL_MEDIA_CATALOG>(
					cdda,
					idx,
				) {
					raw.replace(read);
					break;
				}
				// Wait before re-attempting.
				else if i + 1 < SUBCHANNEL_REREADS {
					std::thread::sleep(SUBCHANNEL_REREAD_DELAY);
				}
			}

			// Try the whole buffer first.
			let raw = raw?;
			Barcode::try_from(raw.as_slice())
				// Fall back to the punctuation-free length.
				.or_else(|_| Barcode::try_from(&raw[..13]))
				.ok()
		}

		// Unsupported?
		if 0 == self.flags & Self::FLAG_SUPPORTS_SUBCHANNEL_MCN {
			std::hint::cold_path();
			None
		}
		else {
			let out = fetch(self, 0);
			if out.is_none() { log!(@trace "Sub-Q contains no MCN data."); }
			out
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

/// # Helper: Capability Flags.
macro_rules! flag {
	( $( $k:ident $v:literal $cap:ident, )+ ) => (
		impl LibcdioInstance {
			$(
				/// # Flag.
				const $k: u8 = $v;
			)+

			#[expect(unsafe_code, reason = "For FFI.")]
			/// # Initialize Capabilities.
			///
			/// Find out whether reading ISRC and/or MCN details from the
			/// subchannel is supported, updating the instance flags
			/// accordingly.
			fn check_capabilities__(&mut self) {
				// Check capabilities.
				// Safety: this is an FFI call…
				let i_read_cap = unsafe {
					let mut i_read_cap = 0;
					let mut i_write_cap = 0;
					let mut i_misc_cap = 0;
					libcdio_sys::cdio_get_drive_cap(
						self.as_ptr(),
						&mut i_read_cap,
						&mut i_write_cap,
						&mut i_misc_cap
					);
					i_read_cap
				};

				$(
					if $cap == i_read_cap & $cap {
						self.flags |= Self::$k;
					}
				)+
			}
		}
	);
}

flag! {
	FLAG_SUPPORTS_SUBCHANNEL_ISRC 0b0001 cdio_drive_cap_read_t_CDIO_DRIVE_CAP_READ_ISRC,
	FLAG_SUPPORTS_SUBCHANNEL_MCN  0b0010 cdio_drive_cap_read_t_CDIO_DRIVE_CAP_READ_MCN,
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

#[expect(unsafe_code, reason = "For FFI.")]
#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
/// # Read MCN/ISRC Subchannel Data.
///
/// Manually read the subchannel data, returning the payload if seemingly
/// successful.
fn read_isrc_mcn_subchannel<const FORMAT: u32>(
	cdda: &LibcdioInstance,
	idx: u8,
) -> Option<[u8; 15]> {
	// Stupid inconsistent integer types. Haha.
	const {
		assert!(
			FORMAT <= u8::MAX as u32,
			"BUG: Subchannel data format must fit `u8`.",
		);
	}

	// Full size is 4 + 20, even though some bytes won't be used.
	let mut p_buf = [0_i8; 24];
	let mut len = 24_u32;

	// Safety: this is an FFI call.
	if driver_return_code_t_DRIVER_OP_SUCCESS == unsafe {
		libcdio_sys::mmc_read_subchannel(
			cdda.as_ptr(),
			idx,
			FORMAT as u8,
			&raw mut len,
			p_buf.as_mut_ptr(),
			2000,
		)
	} {
		// Validate and extract.
		isrc_mcn_subchannel_payload::<FORMAT>(p_buf, idx)
	}
	// Didn't work.
	else { None }
}

/// # Extract MCN/ISRC Subchannel Payload.
///
/// This method checks the MCVAL/TCVAL bit from a subchannel read buffer and
/// if valid, converts the remaining payload to a fixed byte array.
///
/// Note this does not validate the payload content.
const fn isrc_mcn_subchannel_payload<const FORMAT: u32>(buf: [i8; 24], idx: u8)
-> Option<[u8; 15]> {
	if
		(buf[5].cast_unsigned() >> 4) as u32 == FORMAT && // Right format.
		buf[6].cast_unsigned() == idx &&     // Right track.
		(buf[8].cast_unsigned() & 0x80) != 0 // MCVAL/TCVAL bit indicates validity.
	{
		// Bytes 9+ are the ones of interest, but we need to convert
		// them to normal-ass bytes.
		Some([
			buf[9].cast_unsigned(),  buf[10].cast_unsigned(), buf[11].cast_unsigned(),
			buf[12].cast_unsigned(), buf[13].cast_unsigned(), buf[14].cast_unsigned(),
			buf[15].cast_unsigned(), buf[16].cast_unsigned(), buf[17].cast_unsigned(),
			buf[18].cast_unsigned(), buf[19].cast_unsigned(), buf[20].cast_unsigned(),
			buf[21].cast_unsigned(), buf[22].cast_unsigned(), buf[23].cast_unsigned(),
		])
	}
	// Didn't work.
	else { None }
}
