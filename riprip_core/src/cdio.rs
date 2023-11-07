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
			else { Ok(raw.abs_diff(0) + u32::from(CD_LEADIN)) }
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
	#[allow(unsafe_code)]
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
}

impl LibcdioInstance {
	/// # Cache Bust.
	///
	/// There is no simple, universal command to disable or flush a drive's
	/// read buffer, so we have to do the next best thing: fill it with
	/// useless crap!
	///
	/// There is _also_ no good way to know how much crap we need to fill,
	/// because that would be too easy. Haha. Instead we'll just assume the
	/// buffer is 4MiB, and read a teenie bit more than that. That should cover
	/// most drives.
	///
	/// Thankfully, we're never reading the same sector back-to-back, so this
	/// only has to be done once per track, not after each and every read.
	///
	/// For this to work, we have to be able to find regions outside the track
	/// range. That should usually be possible, but won't _always_ be.
	/// Sometimes we'll just have to live with cache.
	///
	/// Also of note: drives tend to slow down for read errors. This will
	/// skip any sector which previously returned a read error to keep it from
	/// being too terrible.
	pub(super) fn cache_bust(
		&self,
		buf: &mut[u8],
		mut todo: u32,
		rng: &Range<i32>,
		leadout: i32,
		backwards: bool,
		killed: &KillSwitch,
	) {
		if 0 != todo && buf.len() == usize::from(CD_DATA_SIZE) {
			let now = Instant::now();

			// If we're moving backwards, try after, then before.
			if backwards {
				self._cache_bust(buf, rng.end, leadout, &mut todo, now, killed);
				self._cache_bust(buf, 0, rng.start - 1, &mut todo, now, killed);
			}
			// Otherwise before, then after.
			else {
				self._cache_bust(buf, 0, rng.start - 1, &mut todo, now, killed);
				self._cache_bust(buf, rng.end, leadout, &mut todo, now, killed);
			}
		}
	}

	/// # Actually Cache Bust.
	///
	/// This method attempts to read up to `todo` sectors between `from..to`.
	/// It is separated from the main method only to cut down on repetitive
	/// code.
	fn _cache_bust(
		&self,
		buf: &mut[u8],
		mut from: i32,
		to: i32,
		todo: &mut u32,
		now: Instant,
		killed: &KillSwitch,
	) {
		while from < to && 0 < *todo {
			if killed.killed() || CACHE_BUST_TIMEOUT < now.elapsed() {
				*todo = 0;
				break;
			}
			if
				! SHITLIST.with_borrow(|q| q.contains(&from)) &&
				self.read_cd(buf, from, false, 0, CD_DATA_SIZE).is_ok()
			{ *todo -= 1; }
			from += 1;
		}
	}

	/// # Read Data + C2.
	///
	/// Read a single sector's worth of data and C2 error pointer information
	/// into the buffer.
	///
	/// ## Errors
	///
	/// This will return an error if the read operation is unsupported or
	/// otherwise fails.
	pub(super) fn read_cd_c2(
		&self,
		buf: &mut [u8; CD_DATA_C2_SIZE as usize],
		lsn: i32,
	) -> Result<(), RipRipError> {
		// We can't read negative, so assume everything is good and null.
		if lsn < 0 {
			for v in buf { *v = 0; }
			return Ok(());
		}

		// Read it!
		self.read_cd(buf, lsn, true, 0, CD_DATA_C2_SIZE)
	}

	#[allow(unsafe_code)]
	/// # Read Data + Subchannel
	///
	/// Read a single sector's worth of data and formatted 16-byte subchannel
	/// information into the buffer. The subchannel data will be parsed to
	/// confirm the timecode matches up with the LSN, where possible, and
	/// trigger a sync error if that fails.
	///
	/// ## Errors
	///
	/// This will return an error if the read operation is unsupported or
	/// otherwise fails, or if the timecode does not match the LSN.
	pub(super) fn read_subchannel(
		&self,
		buf: &mut [u8],
		lsn: i32,
	) -> Result<(), RipRipError> {
		// The buffer and block size are equivalent for our purposes.
		if buf.len() != usize::from(CD_DATA_SUBCHANNEL_SIZE) {
			return Err(RipRipError::Bug("Invalid read buffer size (subchannel)."));
		}

		// We can't read negative, so assume everything is good and null.
		if lsn < 0 {
			for v in &mut *buf { *v = 0; }
			return Ok(());
		}

		// Read it!
		self.read_cd(buf, lsn, false, 2, CD_DATA_SUBCHANNEL_SIZE)?;

		// We can only get timing information from ADR-1.
		if 1 == buf[usize::from(CD_DATA_SIZE)] & 0b0000_1111 {
			// Confirm the subchannel LSN matches the LSN we requested.
			let msf = libcdio_sys::msf_s {
				m: buf[usize::from(CD_DATA_SIZE) + 7],
				s: buf[usize::from(CD_DATA_SIZE) + 8],
				f: buf[usize::from(CD_DATA_SIZE) + 9],
			};
			if lsn != unsafe { libcdio_sys::cdio_msf_to_lsn(&msf) } {
				return Err(RipRipError::SubchannelDesync);
			}
		}

		// As good as we can do!
		Ok(())
	}

	#[allow(unsafe_code)]
	#[allow(non_upper_case_globals)] // Not our globals.
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
		let res = unsafe {
			libcdio_sys::mmc_read_cd(
				self.as_ptr(),
				buf.as_mut_ptr().cast(),
				lsn,
				1,            // Sector type: CDDA.
				0,            // No random data manipulation thank you kindly.
				0,            // No header syncing.
				0,            // No headers.
				1,            // YES audio block!
				0,            // No EDC.
				u8::from(c2), // C2 or no C2?
				sub,          // Subchannel? What kind?
				block_size,   // Block size (varies by data requested).
				1,            // Always read one block at a time.
			)
		};

		match res {
			driver_return_code_t_DRIVER_OP_NOT_PERMITTED => Err(RipRipError::CdReadUnsupported),
			driver_return_code_t_DRIVER_OP_SUCCESS => Ok(()),
			_ => {
				SHITLIST.with(|q| q.borrow_mut().insert(lsn));
				Err(RipRipError::CdRead)
			},
		}
	}
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

#[allow(unsafe_code)]
/// # Initialize `libcdio`.
///
/// This is only called once, but to be safe, it is also wrapped in a static to
/// make sure it can never re-initialize.
fn init() {
	LIBCDIO_INIT.call_once(|| unsafe { libcdio_sys::cdio_init(); });
}
