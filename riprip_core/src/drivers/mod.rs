/*!
# Rip Rip Hooray: CD I/O Drivers.

This module abstracts CD I/O drivers — currently just `libcdio` — to make it
easier for new ones to be added in the future.

At present, this simply exports a single type alias — `CddaDriver` — for use
within the rest of the library, and a corresponding `CddaDriverExt` trait.

Somewhat useful documentation:
<https://www.t10.org/ftp/t10/document.97/97-117r0.pdf>
*/

// TODO: update logic when there are multiple drivers to ensure that only
// one is enabled.
#[cfg(not(feature = "libcdio"))]
compile_error!("Crate feature `libcdio` is required.");

// TODO: suggest using `libusb` feature when merged.
// #[cfg(all(target_os = "macos", feature = "libcdio"))]
// compile_error!("Apple does not fully support `libcdio`.");

#[cfg(feature = "libcdio")]
mod libcdio;

#[cfg(feature = "libusb")]
mod libusb;

use crate::{
	Barcode,
	CD_DATA_C2_SIZE,
	CD_DATA_SIZE,
	CD_DATA_SUBCHANNEL_SIZE,
	CD_LEADIN,
	DriveVendorModel,
	FRAMES_PER_SECOND,
	KillSwitch,
	RipRipError,
};
use dactyl::NoHash;
use std::{
	cell::RefCell,
	cmp::Ordering,
	collections::HashSet,
	fmt,
	path::Path,
	range::legacy::Range,
	time::{
		Duration,
		Instant,
	},
};



#[cfg(all(feature = "libcdio", not(feature = "libusb")))]
/// # CDIO Driver Middleware.
///
/// This type alias is how the rest of the library references the chosen
/// driver.
pub(crate) type CddaDriver = libcdio::LibcdioInstance;

#[cfg(feature = "libusb")]
/// # USB Driver.
///
/// This type alias is how the rest of the library references the chosen
/// driver.
pub(crate) type CddaDriver = libusb::LibusbInstance;


/// # Cache Bust Timeout.
const CACHE_BUST_TIMEOUT: Duration = Duration::from_secs(45);

thread_local! {
	/// # Sector Shitlist.
	///
	/// Keep track of sectors that trigger hard read errors so we don't
	/// accidentally try them in a cache-bust situation.
	static SHITLIST: RefCell<HashSet<i32, NoHash>> = RefCell::new(HashSet::with_hasher(NoHash::default()));
}



/// # Helper: CDText Fields.
macro_rules! fields {
	( $( $k:ident $v:ident $vstr:literal ),+ $(,)? ) => (
		#[repr(u32)]
		#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
		/// # CDText Field.
		///
		/// This enum simply rearranges the constants exported from `libcdio` in a
		/// friendlier format.
		pub enum CDTextKind {
			$(
				#[cfg(feature = "libcdio")]
				#[doc = concat!("# ", stringify!($k), ".")]
				$k = ::libcdio_sys::$v,

				#[cfg(not(feature = "libcdio"))]
				#[doc = concat!("# ", stringify!($k), ".")]
				$k,
			)+
		}

		impl CDTextKind {
			#[must_use]
			/// # As Str.
			///
			/// Return the field as an uppercase string, similar to how it would
			/// appear in track metadata.
			pub const fn as_str(self) -> &'static str {
				match self {
					$( Self::$k => $vstr, )+
				}
			}
		}
	);
}

fields! {
	Arranger   cdtext_field_t_CDTEXT_FIELD_ARRANGER   "ARRANGER",
	Barcode    cdtext_field_t_CDTEXT_FIELD_UPC_EAN    "BARCODE",
	Composer   cdtext_field_t_CDTEXT_FIELD_COMPOSER   "COMPOSER",
	Isrc       cdtext_field_t_CDTEXT_FIELD_ISRC       "ISRC",
	Message    cdtext_field_t_CDTEXT_FIELD_MESSAGE    "COMMENT",
	Performer  cdtext_field_t_CDTEXT_FIELD_PERFORMER  "ARTIST",
	Songwriter cdtext_field_t_CDTEXT_FIELD_SONGWRITER "SONGWRITER",
	Title      cdtext_field_t_CDTEXT_FIELD_TITLE      "TITLE",
}

impl AsRef<str> for CDTextKind {
	#[inline]
	fn as_ref(&self) -> &str { self.as_str() }
}

impl fmt::Display for CDTextKind {
	#[inline]
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		<str as fmt::Display>::fmt(self.as_str(), f)
	}
}

impl Ord for CDTextKind {
	#[inline]
	fn cmp(&self, rhs: &Self) -> Ordering { self.as_str().cmp(rhs.as_str()) }
}

impl PartialOrd for CDTextKind {
	#[inline]
	fn partial_cmp(&self, rhs: &Self) -> Option<Ordering> { Some(self.cmp(rhs)) }
}



/// # CDDA Read Trait.
///
/// This trait is used to help deduplicate code between drivers.
pub(crate) trait CddaDriverExt: Sized {
	/// # New!
	///
	/// Initialize a new instance, optionally connecting to a specific device.
	///
	/// ## Errors
	///
	/// This will return an error if initialization fails, or if the provided
	/// device path is obviously wrong.
	fn new<P>(dev: Option<P>) -> Result<Self, RipRipError>
	where P: AsRef<Path>;

	/// # First Track Number.
	///
	/// Return the first track number on the disc, almost always but not
	/// necessarily `1`.
	fn first_track_num(&self) -> Result<u8, RipRipError>;

	/// # Leadout.
	///
	/// Return the LBA — including the leading `150` — of the disc leadout.
	fn leadout_lba(&self) -> Result<u32, RipRipError>;

	/// # Get the Number of Tracks.
	///
	/// Return the total number of tracks, or the last track number, however
	/// you want to think of it.
	fn num_tracks(&self) -> Result<u8, RipRipError>;

	/// # Track Format.
	///
	/// Returns `true` for audio, `false` for data, and an error for anything
	/// else.
	fn track_format(&self, idx: u8) -> Result<bool, RipRipError>;

	/// # Track LBA Start.
	///
	/// Return the starting LBA — including the leading `150` — for a given
	/// track.
	fn track_lba_start(&self, idx: u8) -> Result<u32, RipRipError>;

	/// # CDText Value.
	///
	/// Return the value associated with the CDText field, if any. If the track
	/// number is zero, data associated with the album will be returned.
	fn cdtext(&self, idx: u8, kind: CDTextKind) -> Option<String>;

	/// # Drive Vendor/Model.
	///
	/// Fetch the drive vendor and/or model, if possible.
	fn drive_vendor_model(&self) -> Option<DriveVendorModel>;

	/// # MCN From (Leadin) Subchannel.
	///
	/// Return the MCN as stored in the leadin subchannel data, if any.
	fn mcn_subchannel(&self) -> Option<Barcode>;

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
	) -> Result<(), RipRipError>;

	/// # MCN.
	///
	/// Return the disc's associated UPC/EAN, if present, either from CDText
	/// or the leadin subchannel data.
	fn mcn(&self) -> Option<Barcode> {
		self.mcn_cdtext().or_else(|| self.mcn_subchannel())
	}

	/// # MCN From CDText.
	///
	/// Return the MCN as stored in the CDText, if any.
	fn mcn_cdtext(&self) -> Option<Barcode> {
		self.cdtext(0, CDTextKind::Barcode)
			.and_then(|v| Barcode::try_from(v.as_bytes()).ok())
	}

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
	fn cache_bust(
		&self,
		buf: &mut[u8],
		mut todo: u32,
		rng: &Range<i32>,
		leadout: i32,
		backwards: bool,
		killed: KillSwitch,
	) {
		if 0 != todo && buf.len() == usize::from(CD_DATA_SIZE) {
			let now = Instant::now();

			// If we're moving backwards, try after, then before.
			if backwards {
				cache_bust(self, buf, rng.end, leadout, &mut todo, now, killed);
				cache_bust(self, buf, 0, rng.start - 1, &mut todo, now, killed);
			}
			// Otherwise before, then after.
			else {
				cache_bust(self, buf, 0, rng.start - 1, &mut todo, now, killed);
				cache_bust(self, buf, rng.end, leadout, &mut todo, now, killed);
			}
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
	fn read_cd_c2(
		&self,
		buf: &mut [u8; CD_DATA_C2_SIZE as usize],
		lsn: i32,
	) -> Result<(), RipRipError> {
		// We can't read negative, so assume everything is good and null.
		if lsn < 0 {
			buf.fill(0);
			return Ok(());
		}

		// Read it!
		self.read_cd(buf, lsn, true, 0, CD_DATA_C2_SIZE)
	}

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
	fn read_subchannel(
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
		if
			1 == buf[usize::from(CD_DATA_SIZE)] & 0b0000_1111 &&
			lsn != msf_to_lsn(
				buf[usize::from(CD_DATA_SIZE) + 7],
				buf[usize::from(CD_DATA_SIZE) + 8],
				buf[usize::from(CD_DATA_SIZE) + 9],
			)
		{
			return Err(RipRipError::SubchannelDesync);
		}

		// As good as we can do!
		Ok(())
	}
}



#[must_use]
/// # Is Bad Sector?
///
/// Returns `true` if `lsn` is in the `SHITLIST`.
fn bad_sector(lsn: i32) -> bool {
	SHITLIST.with_borrow(|q| q.contains(&lsn))
}

/// # Cache Bust.
///
/// This is a generic helper for the default `CddaDriverExt::cache_bust`
/// implementation.
fn cache_bust<D: CddaDriverExt>(
	driver: &D,
	buf: &mut[u8],
	mut from: i32,
	to: i32,
	todo: &mut u32,
	now: Instant,
	killed: KillSwitch,
) {
	while from < to && 0 < *todo {
		if killed.killed() || CACHE_BUST_TIMEOUT < now.elapsed() {
			*todo = 0;
			break;
		}

		if ! bad_sector(from) && driver.read_cd(buf, from, false, 0, CD_DATA_SIZE).is_ok() {
			*todo -= 1;
		}

		from += 1;
	}
}

#[must_use]
/// # MSF to LSN.
///
/// Convert minutes, seconds, and frames to a logical sector number.
const fn msf_to_lsn(m: u8, s: u8, f: u8) -> i32 {
	((m as i32) * 60 * (FRAMES_PER_SECOND as i32)) +
	((s as i32) * (FRAMES_PER_SECOND as i32)) +
	(f as i32) -
	(CD_LEADIN as i32)
}

/// # Set Bad Sector.
///
/// Add `lsn` to the `SHITLIST` (for the benefit of future cache-busting
/// exercises).
fn set_bad_sector(lsn: i32) {
	SHITLIST.with(|q| q.borrow_mut().insert(lsn));
}
