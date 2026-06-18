/*!
# Rip Rip Hooray: `libusb` Wrappers

Somewhat useful documentation:
- <https://docs.rs/rusb/0.9.4/rusb/>
- <https://www.13thmonkey.org/documentation/SCSI/mmc1r09.pdf>
*/

use crate::{
    Barcode, CDTextKind, DriveVendorModel, KillSwitch, RipRipError, CD_DATA_C2_SIZE, CD_DATA_SIZE,
    CD_DATA_SUBCHANNEL_SIZE, CD_LEADIN,
};

use dactyl::{traits::SaturatingFrom, NoHash};
use nix::unistd::{setuid, Uid};
use rusb::{
    Context, Device, DeviceHandle, DeviceList, Direction, Error, GlobalContext, TransferType,
    UsbContext,
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

/// The `CommandBlockWrapper` signature.
const CBW_SIGNATURE: u32 = u32::from_le_bytes(*b"USBC");
/// The `CommandStatusWrapper` signature.
const CSW_SIGNATURE: u32 = u32::from_le_bytes(*b"USBS");
const CBW_LEN: usize = 31;
const CSW_LEN: usize = 13;

/// # Cache Bust Timeout.
const CACHE_BUST_TIMEOUT: Duration = Duration::from_secs(45);

/// # Write Bulk Timeout.
const WRITE_BULK_TIMEOUT: Duration = Duration::from_secs(2);

/// # Read Bulk Timeout.
const READ_BULK_TIMEOUT: Duration = Duration::from_secs(5);

/// # Status Read Timeout.
const STATUS_READ_TIMEOUT: Duration = Duration::from_secs(2);

thread_local! {
    /// # Sector Shitlist.
    ///
    /// Keep track of sectors that trigger hard read errors so we don't
    /// accidentally try them in a cache-bust situation.
    static SHITLIST: RefCell<HashSet<i32, NoHash>> = RefCell::new(HashSet::with_hasher(NoHash::default()));
}

/// `spc` (SCSI Primary Commands) covers baseline commands that every SCSI device must understand,
/// regardless of what it is (like INQUIRY or TEST_UNIT_READY).
mod spc {
    pub(super) const TEST_UNIT_READY: u8 = 0x00;
    pub(super) const REQUEST_SENSE: u8 = 0x03;
    pub(super) const INQUIRY: u8 = 0x12;
    pub(super) const MODE_SELECT_10: u8 = 0x55;
    pub(super) const MODE_SENSE_10: u8 = 0x5A;
}

/// `mmc` (Multi-Media Commands) covers commands, tracks, and structures specifically unique to
/// optical discs (CDs, DVDs, Blu-rays).
mod mmc {
    pub(super) const READ_SUB_CHANNEL: u8 = 0x42;
    pub(super) const READ_TOC: u8 = 0x43;
    pub(super) const READ_CD: u8 = 0xBE;

    pub(super) const FIRST_TRACK: u8 = 0x01;
    pub(super) const LEAD_OUT: u8 = 0xAA;

    pub(super) const TOC_FORMAT_TOC: u8 = 0x00;
    pub(super) const TOC_FORMAT_SESSION: u8 = 0x01;
    pub(super) const TOC_FORMAT_FULL: u8 = 0x02;
    pub(super) const TOC_FORMAT_PMA: u8 = 0x03;
    pub(super) const TOC_FORMAT_ATIP: u8 = 0x04;

    pub(super) const CTRL_DATA_TRACK: u8 = 0x04; // 1 = Data track, 0 = Audio track

    pub(super) const SUB_FORMAT_MCN: u8 = 0x02;
    pub(super) const SUB_FORMAT_ISRC: u8 = 0x03;
}

#[derive(Debug, Default)]
struct CommandBlockWrapper {
    pub signature: u32,
    pub tag: u32,
    pub data_transfer_length: u32,
    pub flags: u8,
    pub lun: u8,
    pub cb_length: u8,
    pub cdb: [u8; 16],
}

impl CommandBlockWrapper {
    fn to_bytes(&self) -> [u8; CBW_LEN] {
        let mut buf = [0u8; CBW_LEN];
        buf[0..4].copy_from_slice(&self.signature.to_le_bytes());
        buf[4..8].copy_from_slice(&self.tag.to_le_bytes());
        buf[8..12].copy_from_slice(&self.data_transfer_length.to_le_bytes());
        buf[12] = self.flags;
        buf[13] = self.lun;
        buf[14] = self.cb_length;
        buf[15..31].copy_from_slice(&self.cdb);
        buf
    }
}

#[derive(Debug, Default)]
struct CommandStatusWrapper {
    pub signature: u32,
    pub tag: u32,
    pub data_residue: u32,
    pub status: u8,
}

impl CommandStatusWrapper {
    fn from_bytes(buf: &[u8; CSW_LEN]) -> Self {
        Self {
            signature: u32::from_le_bytes(buf[0..4].try_into().unwrap()),
            tag: u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            data_residue: u32::from_le_bytes(buf[8..12].try_into().unwrap()),
            status: buf[12],
        }
    }
}

#[derive(Debug, Default)]
struct Endpoints {
    pub bulk_in: u8,
    pub bulk_out: u8,
}

fn detect_bulk_endpoints<T: UsbContext>(device: &Device<T>) -> Result<Endpoints, rusb::Error> {
    let mut endpoints = Endpoints::default();

    // 1. Get the active configuration descriptor
    let config_desc = device.active_config_descriptor()?;

    // 2. Iterate through interfaces (Mass Storage is typically Interface 0)
    for interface in config_desc.interfaces() {
        // 3. Look at the primary setting (Setting 0)
        for interface_desc in interface.descriptors() {
            // Optional: You can verify this is a Mass Storage Interface
            // Class 0x08 = Mass Storage, SubClass 0x06 = SCSI Transparent, Protocol 0x50 = Bulk-Only (BOT)
            if interface_desc.class_code() == 0x08 {
                // Found Mass Storage interface! Let's parse its endpoints.
                for endpoint_desc in interface_desc.endpoint_descriptors() {
                    // 4. Check if the endpoint transfer type is BULK
                    if endpoint_desc.transfer_type() == TransferType::Bulk {
                        let address = endpoint_desc.address();

                        // 5. Check the direction bitmask
                        match endpoint_desc.direction() {
                            Direction::In => {
                                endpoints.bulk_in = address;
                            }
                            Direction::Out => {
                                endpoints.bulk_out = address;
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(endpoints)
}

/// # USB Instance.
///
/// Pretty much all CD-related communications run through a single `LibusbInstance`
/// object.
pub(super) struct LibusbInstance<C: UsbContext = GlobalContext> {
    /// # Device Handle.
    device_handle: DeviceHandle<C>,

    interface_id: u8,

    endpoints: Endpoints,

    cbw_tag: AtomicU32,
}

impl<C: UsbContext> Drop for LibusbInstance<C> {
    fn drop(&mut self) {
        if let Err(e) = self.device_handle.release_interface(self.interface_id) {
            eprintln!("Error releasing USB interface {}: {}", self.interface_id, e);
        }
    }
}

fn find_and_open_device<C: UsbContext>(
    devices: DeviceList<C>,
    pid: u16,
    vid: u16,
) -> Result<DeviceHandle<C>, RipRipError> {
    devices
        .iter()
        .find_map(|device| {
            // If descriptor fails, skip to the next device
            let desc = device.device_descriptor().ok()?;

            if desc.product_id() == pid && desc.vendor_id() == vid {
                Some(
                    device
                        .open()
                        .map_err(|e| RipRipError::DeviceOpen(Some(e.to_string()))),
                )
            } else {
                None
            }
        })
        .unwrap_or(Err(RipRipError::DeviceOpen(Some(format!("{pid}:{vid}")))))
}

fn find_and_open_cd_drive<C: UsbContext>(
    devices: DeviceList<C>,
) -> Result<DeviceHandle<C>, RipRipError> {
    devices
        .iter()
        .find_map(|device| {
            let config_desc = device.active_config_descriptor().ok()?;

            let is_cd_drive = config_desc.interfaces().any(|interface| {
                interface.descriptors().any(|desc| {
                    let class = desc.class_code();
                    let subclass = desc.sub_class_code();
                    let protocol = desc.protocol_code();

                    // Class 0x08 = Mass Storage
                    // Subclass 0x02/0x05 = CD-ROM, 0x06 = SCSI Transparent (common for USB-SATA bridges)
                    // Protocol 0x50 = Bulk-Only Transport
                    class == 0x08
                        && (subclass == 0x02 || subclass == 0x05 || subclass == 0x06)
                        && protocol == 0x50
                })
            });

            if is_cd_drive {
                Some(
                    device
                        .open()
                        .map_err(|e| RipRipError::DeviceOpen(Some(e.to_string()))),
                )
            } else {
                None
            }
        })
        .unwrap_or(Err(RipRipError::DeviceOpen(None)))
}

impl<C: UsbContext> LibusbInstance<C> {
    pub(super) fn with_context(
        context: C,
        device: Option<(u16, u16)>,
    ) -> Result<Self, RipRipError> {
        let devices = context
            .devices()
            .map_err(|e| RipRipError::DeviceOpen(Some(e.to_string())))?;

        let device_handle = if let Some((pid, vid)) = device {
            find_and_open_device(devices, pid, vid)?
        } else {
            find_and_open_cd_drive(devices)?
        };

        let endpoints = detect_bulk_endpoints(&device_handle.device()).unwrap();

        let interface_id = 0;

        // Check if kernel driver is owning our device and detach it if so
        if let Ok(true) = device_handle.kernel_driver_active(interface_id) {
            device_handle
                .detach_kernel_driver(interface_id)
                .map_err(|e| RipRipError::Device(e.to_string()))?;
        }

        device_handle.claim_interface(interface_id).unwrap();

        if let Ok(sudo_uid_str) = env::var("SUDO_UID") {
            let original_uid: u32 = sudo_uid_str.parse().unwrap();

            // Drop privileges
            setuid(Uid::from_raw(original_uid)).unwrap();
        }

        Ok(Self {
            device_handle,
            interface_id,
            endpoints,
            cbw_tag: AtomicU32::new(0x10000001),
        })
    }
}

impl LibusbInstance<GlobalContext> {
    /// # New!
    ///
    /// Initialize a new instance, optionally connecting to a specific device.
    ///
    /// This will return an error if initialization fails, or if the provided
    /// vendor and product ids are obviously wrong.
    pub(super) fn new_global(device: Option<(u16, u16)>) -> Result<Self, RipRipError> {
        Self::with_context(GlobalContext::default(), device)
    }
}

impl<T: UsbContext> LibusbInstance<T> {
    /// Helper to send a SCSI MMC command via USB Bulk-Only Transport (BOT)
    /// and read back the resulting data payload.
    fn exec_scsi_read(&self, cmd: &[u8], buf: &mut [u8]) -> Result<(), RipRipError> {
        // Read and increment the local counter attached directly to this specific drive.
        let current_tag = self.cbw_tag.fetch_add(1, Ordering::SeqCst);
        let data_len = buf.len();

        let mut cbw = CommandBlockWrapper {
            signature: CBW_SIGNATURE,
            tag: current_tag,
            data_transfer_length: data_len as u32,
            flags: 0x80, // Device-to-Host
            lun: 0,
            cb_length: cmd.len() as u8,
            cdb: [0u8; 16],
        };
        cbw.cdb[..cmd.len()].copy_from_slice(&cmd);

        let cbw_bytes = cbw.to_bytes();
        self.device_handle
            .write_bulk(self.endpoints.bulk_out, &cbw_bytes, WRITE_BULK_TIMEOUT)
            .map_err(|_| RipRipError::CdRead)?;

        if data_len > 0 {
            let data_res =
                self.device_handle
                    .read_bulk(self.endpoints.bulk_in, buf, READ_BULK_TIMEOUT);

            if let Err(rusb::Error::Pipe) = data_res {
                self.device_handle
                    .clear_halt(self.endpoints.bulk_in)
                    .map_err(|_| RipRipError::CdRead)?;
            } else {
                data_res.map_err(|_| RipRipError::CdRead)?;
            }
        }

        let mut csw_raw = [0u8; CSW_LEN];
        self.device_handle
            .read_bulk(self.endpoints.bulk_in, &mut csw_raw, STATUS_READ_TIMEOUT)
            .map_err(|_| RipRipError::CdRead)?;

        let csw = CommandStatusWrapper::from_bytes(&csw_raw);

        // Verify protocol sync state against our local tag.
        if csw.signature != CSW_SIGNATURE || csw.tag != current_tag {
            return Err(RipRipError::Bug(
                "Fatal Protocol Desync: CSW validation error.",
            ));
        }

        if csw.status != 0 {
            return Err(RipRipError::CdRead);
        }

        Ok(())
    }

    fn get_toc_header(&self) -> Result<(u8, u8), RipRipError> {
        let mut cmd = [0u8; 12];
        cmd[0] = mmc::READ_TOC;
        cmd[1] = 0x00; // 0x00 = Native LBA Format
        cmd[2] = mmc::TOC_FORMAT_TOC; // Format 0: Standard Table of Contents
        cmd[6] = mmc::FIRST_TRACK; // Start reading starting from Track 1

        // Enforce the 12-byte allocation length across bytes 7 and 8 (Big-Endian u16)
        // This ensures the drive returns the header + the first track descriptor without stalling.
        let alloc_len: u16 = 12;
        cmd[7..9].copy_from_slice(&alloc_len.to_be_bytes());

        let mut buf = [0u8; 12];
        self.exec_scsi_read(&cmd, &mut buf)?;

        let first_track = buf[2];
        let last_track = buf[3];

        Ok((first_track, last_track))
    }

    fn get_track_descriptor(&self, idx: u8) -> Result<(u8, u32), RipRipError> {
        let mut cmd = [0u8; 12];
        cmd[0] = mmc::READ_TOC;
        cmd[1] = 0x00; // 0x00 = Native LBA Format
        cmd[2] = mmc::TOC_FORMAT_TOC; // Format 0: Standard Table of Contents
        cmd[6] = idx;

        // 4 bytes for the TOC response header + 8 bytes for a single track descriptor entry.
        let alloc_len: u16 = 12;
        cmd[7..9].copy_from_slice(&alloc_len.to_be_bytes());

        let mut buf = [0u8; 12];
        self.exec_scsi_read(&cmd, &mut buf)?;

        let control_adr = buf[5];
        let lba = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);

        Ok((control_adr, lba))
    }
}

impl<C: rusb::UsbContext> LibusbInstance<C> {
    /// # First Track Number.
    ///
    /// Return the first track number on the disc, almost always but not
    /// necessarily `1`.
    pub(super) fn first_track_num(&self) -> Result<u8, RipRipError> {
        let (first, _) = self.get_toc_header()?;

        if first == 0 {
            Err(RipRipError::FirstTrackNum)
        } else {
            Ok(first)
        }
    }

    /// # Leadout.
    ///
    /// Return the LBA — including the leading `150` — of the disc leadout.
    pub(super) fn leadout_lba(&self) -> Result<u32, RipRipError> {
        // In the SCSI MMC specification, the leadout track information is
        // explicitly queried using the standard magic track index 0xAA.
        self.track_lba_start(mmc::LEAD_OUT)
    }

    /// # Get the Number of Tracks.
    ///
    /// Return the total number of tracks, or the last track number, however
    /// you want to think of it.
    pub(super) fn num_tracks(&self) -> Result<u8, RipRipError> {
        let (first, last) = self.get_toc_header()?;

        if last == 0 {
            Err(RipRipError::NumTracks)
        } else {
            // Handles discs that might not explicitly start at track 1
            Ok(last - first + 1)
        }
    }

    /// # Track Format.
    ///
    /// Returns `true` for audio, `false` for data, and an error for anything
    /// else.
    pub(super) fn track_format(&self, idx: u8) -> Result<bool, RipRipError> {
        let (control_adr, _) = self
            .get_track_descriptor(idx)
            .map_err(|_| RipRipError::TrackFormat(idx))?;

        // In SCSI MMC TOC structures, the 4-bit CONTROL field dictates data types.
        // Bit 2 (0x04) is set if the track is a data track, and clear if it's audio.
        let is_data = (control_adr & mmc::CTRL_DATA_TRACK) > 0;

        Ok(!is_data)
    }

    /// # Track LBA Start.
    ///
    /// Return the starting LBA — including the leading `150` — for a given
    /// track.
    pub(super) fn track_lba_start(&self, idx: u8) -> Result<u32, RipRipError> {
        if idx == 0 {
            return Err(RipRipError::TrackNumber(0));
        }

        let (_, lba) = self
            .get_track_descriptor(idx)
            .map_err(|_| RipRipError::TrackLba(idx))?;

        if lba < 0 {
            Err(RipRipError::TrackNumber(idx))
        } else {
            Ok(lba + u32::from(CD_LEADIN))
        }
    }
}

impl<C: UsbContext> LibusbInstance<C> {
    /// # CDText Value.
    ///
    pub(super) fn cdtext(&self, _idx: u8, _kind: CDTextKind) -> Option<String> {
        // Implementation depends on whether we run a raw lead-in scan during boot.
        // For standard direct SCSI layouts, this acts as an interface lookup hook.
        None
    }

    /// # Track ISRC.
    ///
    /// Fetches the International Standard Recording Code directly from the sub-Q channel
    /// using SCSI opcode 0x42 (READ SUB-CHANNEL).
    pub(super) fn track_isrc(&self, idx: u8) -> Option<String> {
        let mut cmd = [0u8; 12];
        cmd[0] = mmc::READ_SUB_CHANNEL;
        cmd[1] = 0x02; // MSF Address Mode format flag (Bit 1 set)
        cmd[2] = 0x40; // Sub-Q Channel Data Enable (Bit 6 set)
        cmd[3] = mmc::SUB_FORMAT_ISRC; // Data Format: 0x03 (International Standard Recording Code)
        cmd[6] = idx; // Target Track index parameter

        cmd[8] = 24; // Allocation Length: 24 Bytes allocation footprint

        let mut buf = [0u8; 24];
        self.exec_scsi_read(&cmd, &mut buf).ok()?;

        // Verify sub-channel execution parameters
        let data_format = buf[3];
        let subq_element_valid = buf[4]; // Sub-Q channel data status indicator flag

        // If the drive confirms sub-Q tracking sync data exists
        if data_format == mmc::SUB_FORMAT_ISRC && subq_element_valid == 0x01 {
            let is_isrc_valid = (buf[12] & 0x80) != 0; // Bit 7 maps existence state
            if is_isrc_valid {
                // Raw string slice extraction out of the fixed-offset 12-byte block
                let raw_ascii = &buf[13..25];
                return String::from_utf8(raw_ascii.to_vec())
                    .ok()
                    .map(|s| s.trim().to_string());
            }
        }
        None
    }

    /// # MCN.
    ///
    /// Return the disc's associated UPC/EAN barcode, if present. Evaluates local caches
    /// first, then fires a dedicated hardware request sequence.
    pub(super) fn mcn(&self) -> Option<Barcode> {
        if let Some(barcode_str) = self.cdtext(0, CDTextKind::Barcode) {
            if let Ok(barcode) = Barcode::try_from(barcode_str.as_bytes()) {
                return Some(barcode);
            }
        }

        self.mcn__()
    }

    /// # MCN Fallback.
    ///
    /// Pulls the absolute Media Catalog Number via explicit SCSI sub-channel reads.
    fn mcn__(&self) -> Option<Barcode> {
        let mut cmd = [0u8; 12];
        cmd[0] = mmc::READ_SUB_CHANNEL; // Opcode: READ SUB-CHANNEL
        cmd[1] = 0x02; // MSF Address mode format flag
        cmd[2] = 0x40; // Sub-Q Channel tracking bit
        cmd[3] = mmc::SUB_FORMAT_MCN; // Data Format: 0x02 (Media Catalog Number)

        // Request 26 bytes (Standard Sub-channel header + MCN data block size)
        cmd[8] = 26;

        let mut buf = [0u8; 26];
        self.exec_scsi_read(&cmd, &mut buf).ok()?;

        let data_format = buf[3];
        let subq_element_valid = buf[4];

        if data_format == mmc::SUB_FORMAT_MCN && subq_element_valid == 0x01 {
            // Bit 7 tracks string validation rules (MCVAL flag in MMC spec)
            let is_mcn_valid = (buf[12] & 0x80) != 0;
            if is_mcn_valid {
                let raw_ascii = &buf[13..26];
                return Barcode::try_from(raw_ascii).ok();
            }
        }
        None
    }
}

impl<C: UsbContext> LibusbInstance<C> {
    /// # Drive Vendor/Model.
    ///
    /// Fetch the drive vendor and/or model, if possible.
    pub(super) fn drive_vendor_model(&self) -> Option<DriveVendorModel> {
        let mut cmd = [0u8; 12];
        cmd[0] = spc::INQUIRY;
        cmd[4] = 36; // Allocation Length: Standard INQUIRY data size is 36 bytes

        let mut buf = [0u8; 36];
        self.exec_scsi_read(&cmd, &mut buf).ok()?;

        // Standard SCSI Inquiry layout maps fields at fixed offsets:
        // Bytes 8..16  -> Vendor Identification (8 bytes)
        // Bytes 16..32 -> Product Identification / Model (16 bytes)
        let vendor_raw = &buf[8..16];
        let model_raw = &buf[16..32];

        // Convert the raw bytes into UTF-8 strings, stripping away any
        // trailing whitespace padding added by the drive firmware.
        let vendor_str = std::str::from_utf8(vendor_raw).ok()?.trim();

        let model_str = std::str::from_utf8(model_raw).ok()?.trim();

        // Model is required, Vendor might be empty strings
        if model_str.is_empty() {
            return None;
        }

        DriveVendorModel::new(vendor_str, model_str).ok()
    }
}

impl<C: rusb::UsbContext> LibusbInstance<C> {
    /// # Cache Bust.
    ///
    pub(super) fn cache_bust(
        &self,
        buf: &mut [u8],
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
                self.cache_bust__(buf, rng.end, leadout, &mut todo, now, killed);
                self.cache_bust__(buf, 0, rng.start - 1, &mut todo, now, killed);
            }
            // Otherwise before, then after.
            else {
                self.cache_bust__(buf, 0, rng.start - 1, &mut todo, now, killed);
                self.cache_bust__(buf, rng.end, leadout, &mut todo, now, killed);
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
        now: Instant,
        killed: KillSwitch,
    ) {
        while from < to && 0 < *todo {
            if killed.killed() || CACHE_BUST_TIMEOUT < now.elapsed() {
                *todo = 0;
                break;
            }
            if !SHITLIST.with_borrow(|q| q.contains(&from))
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
    pub(super) fn read_cd_c2(
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
    pub(super) fn read_subchannel(&self, buf: &mut [u8], lsn: i32) -> Result<(), RipRipError> {
        if buf.len() != usize::from(CD_DATA_SUBCHANNEL_SIZE) {
            return Err(RipRipError::Bug("Invalid read buffer size (subchannel)."));
        }

        // We can't read negative, so assume everything is good and null.
        if lsn < 0 {
            buf.fill(0);
            return Ok(());
        }

        // Read it!
        // Subchannel parameter 2 maps to selection choice: "Raw Subchannel data (16 Bytes)"
        self.read_cd(buf, lsn, false, 2, CD_DATA_SUBCHANNEL_SIZE)?;

        // We can only get timing information from ADR-1 (check lower nibble of control byte)
        if 1 == (buf[usize::from(CD_DATA_SIZE)] & 0x0F) {
            // Extract MSF (Minutes, Seconds, Frames) timecodes out of subchannel payload data indices
            let m = buf[usize::from(CD_DATA_SIZE) + 7];
            let s = buf[usize::from(CD_DATA_SIZE) + 8];
            let f = buf[usize::from(CD_DATA_SIZE) + 9];

            // Native safe safe-checked recalculation to convert MSF format to LSN offset bounds.
            // Formula: (M * 60 * 75) + (S * 75) + F - 150
            let parsed_lsn = (i32::from(m) * 60 * 75) + (i32::from(s) * 75) + i32::from(f) - 150;

            if lsn != parsed_lsn {
                return Err(RipRipError::SubchannelDesync);
            }
        }

        Ok(())
    }

    /// # Execute Read Command.
    ///
    /// Emits a raw 12-byte SCSI READ CD (0xBE) packet over the USB pipeline.
    #[inline]
    fn read_cd(
        &self,
        buf: &mut [u8],
        lsn: i32,
        c2: bool,
        sub: u8,
        _block_size: u16,
    ) -> Result<(), RipRipError> {
        let mut cdb = [0u8; 12];
        cdb[0] = mmc::READ_CD;
        cdb[1] = 0x04; // Expected Sector Type field flag -> 0x04 means CD-DA Audio

        // riprip's addressing parameters are already absolute LBAs.
        let lba = lsn as u32;
        cdb[2..6].copy_from_slice(&lba.to_be_bytes());

        // Transfer exactly 1 sector at a time
        cdb[6..9].copy_from_slice(&1u32.to_be_bytes()[1..4]);

        // Byte 9 is the Selection Field flag byte:
        // Bit 4: User Data Selection (Set to 1 to read the 2352 bytes audio payload)
        // Bit 2..1: C2 Error Flag selection allocation (10b means include 294 bytes C2 space)
        let user_data_flag = 0x10;
        let c2_flag = if c2 { 0x02 } else { 0x00 };
        cdb[9] = user_data_flag | c2_flag;

        // Byte 10 defines the Sub-channel Selection configuration flags:
        // 0x00 = No sub-channel data requested
        // 0x02 = Raw Subchannel Data payload (16 bytes payload space)
        cdb[10] = sub;

        // Dispatch via your battle-tested SCSI core runner wrapper
        match self.exec_scsi_read(&cdb, buf) {
            Ok(()) => Ok(()),
            Err(_) => {
                SHITLIST.with(|q| q.borrow_mut().insert(lsn));
                Err(RipRipError::CdRead)
            }
        }
    }
}
