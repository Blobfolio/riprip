/*!
# Rip Rip Hooray: CD I/O Drivers.

This module abstracts CD I/O drivers — currently just `libcdio` — to make it
easier for new ones to be added in the future.

At present, this simply exports a single type alias — `CddaDriver` — for use
within the rest of the library, and a corresponding `CddaDriverExt` trait.

Somewhat useful documentation:
<https://www.t10.org/ftp/t10/document.97/97-117r0.pdf>
*/

#[cfg(not(any(feature = "libcdio", feature = "libusb")))]
compile_error!("A driver feature is required. Enable `libcdio` or `libusb`.");

#[cfg(all(feature = "libcdio", feature = "libusb"))]
compile_error!("Conflicting driver features detected. Choose either `libcdio` or `libusb`.");

#[cfg(all(target_os = "macos", feature = "libcdio"))]
compile_error!("The `libcdio` feature does not work on Apple devices. Build with `cargo build --no-default-features --features libusb` instead.");

#[cfg(all(not(target_os = "linux"), not(target_os = "macos"), feature = "libusb"))]
compile_error!("The `libusb` feature requires linux or macos.");



#[cfg(feature = "libcdio")]
mod libcdio;

#[cfg(feature = "libusb")]
mod libusb;

#[cfg(feature = "libusb")]
mod mmc;



use cdtoc::Track;
use crate::{
	Barcode,
	CD_DATA_C2_SIZE,
	CD_DATA_SIZE,
	CD_DATA_SUBCHANNEL_SIZE,
	CD_LEADIN,
	CD_SUBCHANNEL_SIZE,
	DriveVendorModel,
	FRAMES_PER_SECOND,
	Isrc,
	KillSwitch,
	macros::log,
	RipRipError,
	SubQ,
};
use dactyl::NoHash;
use std::{
	cell::RefCell,
	collections::HashSet,
	num::NonZeroU8,
	path::Path,
	range::legacy::Range,
	time::{
		Duration,
		Instant,
	},
};



#[cfg(feature = "libcdio")]
/// # CD/IO Driver Middleware.
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



/// # CDDA Read Trait.
///
/// This trait is used to help deduplicate code between drivers.
pub(crate) trait CddaDriverExt: Sized {
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

	/// # CD-Text (Raw).
	///
	/// Read and return the raw CD-Text data, if any.
	fn cdtext(&self) -> Option<Vec<u8>>;

	/// # Drive Vendor/Model.
	///
	/// Fetch the drive vendor and/or model, if possible.
	fn drive_vendor_model(&self) -> Option<DriveVendorModel>;

	/// # Execute Read Command.
	///
	/// This private method executes the million-argument MMC read command with
	/// values prepared and verified by the caller.
	///
	/// ## Errors.
	///
	/// This will return an error if the read fails, but provides no other
	/// sanity checks.
	fn read_cd(&self, buf: &mut [u8], lsn: i32, opts: ReadCdOpts)
	-> Result<(), RipRipError>;

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
			log!(@trace "Invalid LSN {lsn}; filling buffer with zeroes.");
			buf.fill(0);
			return Ok(());
		}

		// Read it!
		self.read_cd(buf, lsn, ReadCdOpts::CddaPlusC2)
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
			std::hint::cold_path();
			return Err(RipRipError::Bug("Invalid read buffer size (subchannel)."));
		}

		// We can't read negative, so assume everything is good and null.
		if lsn < 0 {
			for v in &mut *buf { *v = 0; }
			return Ok(());
		}

		// Read it!
		self.read_cd(buf, lsn, ReadCdOpts::CddaPlusSubchannel)?;

		// If the subchannel data is valid and ADR-1 _and_ the resulting MSF
		// mismatches what we expected, desync!
		if
			let Some(subq) = buf.last_chunk::<{CD_SUBCHANNEL_SIZE as usize}>() &&
			let Some(SubQ::Timing([_, _, _, _, _, _, m, s, f])) = SubQ::new(subq) &&
			lsn != msf_to_lsn(m, s, f)
		{
			Err(RipRipError::SubchannelDesync)
		}
		// As good as we can do!
		else { Ok(()) }
	}

	/// # Read ISRC.
	///
	/// Pull Sub-Q data from the track, parsing and returning the first valid
	/// ISRC, if any.
	fn read_isrc(&self, track: Track) -> Option<Isrc> {
		if track.is_htoa() { return None; }

		let rng = track.sector_range_normalized();
		let mut start = i32::try_from(rng.start).ok()?;
		let end = i32::try_from(rng.end).ok()?;

		// Prefer the middle of the track.
		if start + 512 < end {
			start = start.midpoint(end) - 128;
		}

		// Should be able to read en masse for these.
		let mut already = HashSet::<[u8; 8]>::with_capacity(256);
		let mut buf = [[0_u8; CD_SUBCHANNEL_SIZE as usize]; 16];
		for _ in 0..16 {
			if self.read_cd(buf.as_flattened_mut(), start, ReadCdOpts::Subchannel).is_ok() {
				for chunk in buf {
					if
						let Some(SubQ::Isrc(subq)) = SubQ::new(&chunk) &&
						already.insert(*subq[..8].as_array().unwrap()) &&
						let Some(isrc) = Isrc::from_subq(subq)
					{
						return Some(isrc);
					}
				}
			}

			start += 16;
		}

		None
	}

	/// # Read MCN.
	///
	/// Pull Sub-Q data from the start of the disc, parsing and returning the
	/// first valid MCN entry, if any.
	fn read_mcn(&self) -> Option<Barcode> {
		// Should be able to read en masse for these.
		let mut start = 256;
		let mut already = HashSet::<[u8; 7]>::with_capacity(256);
		let mut buf = [[0_u8; CD_SUBCHANNEL_SIZE as usize]; 16];
		for _ in 0..16 {
			if self.read_cd(buf.as_flattened_mut(), start, ReadCdOpts::Subchannel).is_ok() {
				for chunk in buf {
					if
						let Some(SubQ::Mcn(subq)) = SubQ::new(&chunk) &&
						already.insert(*subq[..7].as_array().unwrap()) &&
						let Some(barcode) = Barcode::from_subq(subq)
					{
						return Some(barcode);
					}
				}
			}

			start += 16;
		}

		None
	}
}



/// # CDDA Construction Trait.
///
/// This trait provides driver construction.
pub(crate) trait CddaDriverNewExt: Sized {
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
}



/// # Helper: Read CD Configuration.
macro_rules! opts {
	(
		$(
			$( #[doc = $doc:expr] )*
			$k:ident $cd:literal $c2:literal $sub:literal $block_size:ident,
		)+
	) => (
		#[repr(u16)]
		#[derive(Debug, Clone, Copy, Eq, PartialEq)]
		/// # Read CD Options.
		///
		/// This enum is used to limit the number of arguments required for
		/// CD-reading calls. (They all kinda bleed together.)
		pub(crate) enum ReadCdOpts {
			$(
				$( #[doc = $doc] )*
				$k = $block_size,
			)+
		}

		impl ReadCdOpts {
			#[must_use]
			/// # CDDA/User Data?
			const fn cdda(self) -> bool {
				match self {
					$( Self::$k => $cd, )+
				}
			}

			#[must_use]
			/// # C2?
			const fn c2(self) -> bool {
				match self {
					$( Self::$k => $c2, )+
				}
			}

			#[must_use]
			/// # Subchannel Format?
			const fn subchannel_format(self) -> u8 {
				match self {
					$( Self::$k => $sub, )+
				}
			}
		}

		impl ReadCdOpts {
			#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
			/// # Number of Blocks.
			///
			/// Calculate and return the number of blocks required to fill the
			/// buffer, or an error if it does not divide evenly or overflows.
			///
			/// ## Errors
			///
			/// Buffer lengths are baked-in, so any error should be treated as
			/// a bug!
			const fn num_blocks(self, buf: &[u8]) -> Result<NonZeroU8, &'static str> {
				if buf.is_empty() {
					Err("Read CD buffer is empty.")
				}
				else if ! buf.len().is_multiple_of(self as usize) {
					Err("Read CD buffer length is not multiple of block size.")
				}
				else {
					let num_blocks = buf.len() / (self as usize);
					if
						num_blocks <= (u8::MAX as usize) &&
						let Some(num_blocks) = NonZeroU8::new(num_blocks as u8)
					{
						Ok(num_blocks)
					}
					else {
						Err("Read CD buffer length and block size combination require too many blocks.")
					}
				}
			}
		}
	)
}

opts! {
	/// # CDDA.
	Cdda               true  false 0 CD_DATA_SIZE,

	/// # CDDA + C2.
	CddaPlusC2         true  true  0 CD_DATA_C2_SIZE,

	/// # CDDA + Subchannel.
	CddaPlusSubchannel true  false 2 CD_DATA_SUBCHANNEL_SIZE,

	/// # Subchannel.
	Subchannel         false false 2 CD_SUBCHANNEL_SIZE,
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

		if ! bad_sector(from) && driver.read_cd(buf, from, ReadCdOpts::Cdda).is_ok() {
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
	/// # Binary-Coded Decimal Conversion.
	const fn from_bcd8(v: u8) -> i32 {
		let v = v as i32;
		(v & 0x0F) + ((v >> 4) * 10)
	}

	// Convert to LBA.
	let mut lba = from_bcd8(m);
	lba *= 60;                       // Minutes to seconds.
	lba += from_bcd8(s);
	lba *= FRAMES_PER_SECOND as i32; // Seconds to frames.
	lba += from_bcd8(f);

	// Convert to LSN.
	lba - (CD_LEADIN as i32)
}

/// # Set Bad Sector.
///
/// Add `lsn` to the `SHITLIST` (for the benefit of future cache-busting
/// exercises).
fn set_bad_sector(lsn: i32) {
	SHITLIST.with(|q| if q.borrow_mut().insert(lsn) {
		log!(@trace "Added LSN {lsn} to cache-bust exclusion range.");
	});
}
