/*!
# Rip Rip Hooray: CDDA (Compact Disc Digital Audio) Driver.

This trait acts as an interface to an underlying optical drive driver, abstracting hardware-specific
MMC commands and metadata extraction across implementations like `libcdio` or `libusb`.
*/

use crate::{
    Barcode, CDTextKind, DriveVendorModel, KillSwitch, RipRipError, CD_DATA_C2_SIZE, CD_DATA_SIZE,
    CD_DATA_SUBCHANNEL_SIZE, CD_LEADIN,
};

use std::env;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::{
    cell::RefCell,
    collections::HashSet,
    ffi::{CStr, CString},
    ops::Range,
    os::{raw::c_char, unix::ffi::OsStrExt},
    path::Path,
    sync::Once,
    time::{Duration, Instant},
};

/// # Cache Bust Timeout.
const CACHE_BUST_TIMEOUT: Duration = Duration::from_secs(45);

pub(super) trait Cdda {
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

    /// # MCN.
    ///
    /// Return the disc's associated UPC/EAN, if present. This will try CDText
    /// first since that data is already loaded, and fall back to the direct
    /// `cdio_get_mcn` request if that doesn't work.
    fn mcn(&self) -> Option<Barcode>;

    /// # Drive Vendor/Model.
    ///
    /// Fetch the drive vendor and/or model, if possible.
    fn drive_vendor_model(&self) -> Option<DriveVendorModel>;

    fn is_sector_bad(&self, lsn: i32) -> bool;

    /// # Execute Read Command.
    ///
    /// This method executes the million-argument MMC read command with
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
        buf: &mut [u8],
        mut todo: u32,
        rng: &Range<i32>,
        leadout: i32,
        backwards: bool,
        killed: &KillSwitch,
    ) {
        if 0 != todo && buf.len() == usize::from(CD_DATA_SIZE) {
            let now = Instant::now();
            if backwards {
                self.cache_bust__(buf, rng.end, leadout, &mut todo, &now, killed);
                self.cache_bust__(buf, 0, rng.start - 1, &mut todo, &now, killed);
            } else {
                self.cache_bust__(buf, 0, rng.start - 1, &mut todo, &now, killed);
                self.cache_bust__(buf, rng.end, leadout, &mut todo, &now, killed);
            }
        }
    }

    /// # Actually Cache Bust.
    ///
    /// This method attempts to read up to `todo` sectors between `from..to`.
    /// It is separated from the main method only to cut down on repetitive
    /// code.
    fn cache_bust__(
        &self,
        buf: &mut [u8],
        mut from: i32,
        to: i32,
        todo: &mut u32,
        now: &Instant,
        killed: &KillSwitch,
    ) {
        while from < to && 0 < *todo {
            if killed.killed() || CACHE_BUST_TIMEOUT < now.elapsed() {
                *todo = 0;
                break;
            }
            if !self.is_sector_bad(from)
                && self.read_cd(buf, from, false, 0, CD_DATA_SIZE).is_ok()
            {
                *todo -= 1;
            }
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
    fn read_cd_c2(
        &self,
        buf: &mut [u8; CD_DATA_C2_SIZE as usize],
        lsn: i32,
    ) -> Result<(), RipRipError> {
        if lsn < 0 {
            buf.fill(0);
            return Ok(());
        }
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
    fn read_subchannel(&self, buf: &mut [u8], lsn: i32) -> Result<(), RipRipError> {
        if buf.len() != usize::from(CD_DATA_SUBCHANNEL_SIZE) {
            return Err(RipRipError::Bug("Invalid read buffer size (subchannel)."));
        }

        if lsn < 0 {
            buf.fill(0);
            return Ok(());
        }

        self.read_cd(buf, lsn, false, 2, CD_DATA_SUBCHANNEL_SIZE)?;

        if 1 == (buf[usize::from(CD_DATA_SIZE)] & 0x0F) {
            let m = buf[usize::from(CD_DATA_SIZE) + 7];
            let s = buf[usize::from(CD_DATA_SIZE) + 8];
            let f = buf[usize::from(CD_DATA_SIZE) + 9];

            let parsed_lsn = (i32::from(m) * 60 * 75) + (i32::from(s) * 75) + i32::from(f) - 150;

            if lsn != parsed_lsn {
                return Err(RipRipError::SubchannelDesync);
            }
        }

        Ok(())
    }
}
