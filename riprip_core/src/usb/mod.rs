/*!
# Rip Rip Hooray: `libusb` Wrappers

Somewhat useful documentation:
- <https://docs.rs/rusb/0.9.4/rusb/>
- <https://www.13thmonkey.org/documentation/SCSI/mmc1r09.pdf>
*/

mod bot;
mod cdtext;
mod language;
mod mmc;

use crate::{
    Barcode, CDTextKind, Cdda, DriveVendorModel, KillSwitch, RipRipError, CD_DATA_C2_SIZE,
    CD_DATA_SIZE, CD_DATA_SUBCHANNEL_SIZE, CD_LEADIN,
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

fn find_and_open_device<C: UsbContext>(
    devices: DeviceList<C>,
    vid: u16,
    pid: u16,
) -> Result<DeviceHandle<C>, RipRipError> {
    devices
        .iter()
        .find_map(|device| {
            // If descriptor fails, skip to the next device
            let desc = device.device_descriptor().ok()?;

            if desc.vendor_id() == vid && desc.product_id() == pid {
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
                    desc.class_code() == bot::CLASS_MASS_STORAGE
                        && (bot::OPTICAL_DRIVE_SUBCLASSES.contains(&desc.sub_class_code()))
                        && desc.protocol_code() == bot::PROTOCOL_BULK_ONLY
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

fn detect_bulk_endpoints<T: UsbContext>(device: &Device<T>) -> Result<Endpoints, RipRipError> {
    let config_desc = device
        .active_config_descriptor()
        .map_err(|e| RipRipError::Device(e.to_string()))?;

    let endpoints = config_desc
        .interfaces()
        .flat_map(|iface| iface.descriptors())
        .filter(|iface_desc| iface_desc.class_code() == bot::CLASS_MASS_STORAGE)
        .flat_map(|iface_desc| iface_desc.endpoint_descriptors())
        .filter(|ep_desc| ep_desc.transfer_type() == TransferType::Bulk)
        .fold(Endpoints::default(), |mut acc, ep_desc| {
            match ep_desc.direction() {
                Direction::In => acc.bulk_in = ep_desc.address(),
                Direction::Out => acc.bulk_out = ep_desc.address(),
            }
            acc
        });

    Ok(endpoints)
}

#[derive(Debug, Default)]
struct Endpoints {
    pub bulk_in: u8,
    pub bulk_out: u8,
}

/// # USB Instance.
///
/// Pretty much all CD-related communications run through a single `LibusbInstance`
/// object.
pub(super) struct LibusbInstance<C: UsbContext = GlobalContext> {
    /// # USB Device.
    device_handle: DeviceHandle<C>,

    interface_id: u8,

    endpoints: Endpoints,

    cbw_tag: AtomicU32,

    /// # CD-Text.
    metadata: Option<cdtext::Metadata>,
}

impl<C: UsbContext> Drop for LibusbInstance<C> {
    fn drop(&mut self) {
        if let Err(e) = self.device_handle.release_interface(self.interface_id) {
            eprintln!("Error releasing USB interface {}: {}", self.interface_id, e);
        }

        if let Err(e) = self.device_handle.attach_kernel_driver(self.interface_id) {
            eprintln!(
                "Error reattaching kernel driver for interface {}: {}",
                self.interface_id, e
            );
        }
    }
}

impl<C: UsbContext> LibusbInstance<C> {
    pub(super) fn with_context(
        context: C,
        device: Option<(u16, u16)>,
    ) -> Result<Self, RipRipError> {
        let devices = context
            .devices()
            .map_err(|e| RipRipError::DeviceOpen(Some(e.to_string())))?;

        let device_handle = if let Some((vid, pid)) = device {
            find_and_open_device(devices, vid, pid)?
        } else {
            find_and_open_cd_drive(devices)?
        };

        let endpoints = detect_bulk_endpoints(&device_handle.device())?;

        let interface_id = 0;

        // Check if kernel driver is owning our device and detach it if so
        if let Ok(true) = device_handle.kernel_driver_active(interface_id) {
            device_handle
                .detach_kernel_driver(interface_id)
                .map_err(|e| RipRipError::Device(e.to_string()))?;
        }

        device_handle
            .claim_interface(interface_id)
            .map_err(|_| RipRipError::Bug("Failed to claim iface."))?;

        if let Ok(sudo_uid) = env::var("SUDO_UID") {
            let original_uid: u32 = sudo_uid
                .parse()
                .map_err(|_| RipRipError::Bug("SUDO_UID is not a valid integer."))?;

            setuid(Uid::from_raw(original_uid))
                .map_err(|_| RipRipError::Bug("Failed to drop process privileges."))?;
        }

        let mut out = Self {
            device_handle,
            interface_id,
            endpoints,
            cbw_tag: AtomicU32::new(0x10000001),
            metadata: None,
        };

        // TODO: check for C2Mode296
        out.check_disc_mode__()?;

        if let Some(buf) = out.read_cdtext() {
            let opt = cdtext::Metadata::parse(&buf).map_err(|_| RipRipError::CdText)?;
            if let Some(metadata) = opt {
                out.metadata.replace(metadata);
            }
        }

        Ok(out)
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
        if let Some((vid, pid)) = device {
            println!("{vid:04x}:{pid:04x}");
        }
        Self::with_context(GlobalContext::default(), device)
    }
}

impl<T: UsbContext> LibusbInstance<T> {
    /// Helper to send a SCSI MMC command via USB Bulk-Only Transport (BOT)
    /// and read back the resulting data payload.
    fn exec_scsi_read(&self, cmd: &[u8], buf: &mut [u8]) -> Result<usize, RipRipError> {
        use bot::{
            CommandBlockWrapper, CommandStatusWrapper, CBW_SIGNATURE, CSW_LEN, CSW_SIGNATURE,
        };

        // Read and increment the local counter attached directly to this specific drive.
        let current_tag = self.cbw_tag.fetch_add(1, Ordering::Relaxed);
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
            .map_err(|e| RipRipError::Internal(e.to_string()))?;

        // Skip the read phase entirely if no data transfer is expected.
        let transferred = if data_len > 0 {
            match self
                .device_handle
                .read_bulk(self.endpoints.bulk_in, buf, READ_BULK_TIMEOUT)
            {
                Ok(n) => n,
                Err(rusb::Error::Pipe) => {
                    self.device_handle
                        .clear_halt(self.endpoints.bulk_in)
                        .map_err(|e| RipRipError::Internal(e.to_string()))?;
                    0
                }
                Err(e) => return Err(RipRipError::Internal(e.to_string())),
            }
        } else {
            0
        };

        let mut csw_raw = [0u8; CSW_LEN];
        let len = self
            .device_handle
            .read_bulk(self.endpoints.bulk_in, &mut csw_raw, STATUS_READ_TIMEOUT)
            .map_err(|e| RipRipError::Internal(e.to_string()))?;

        if len != CSW_LEN {
            return Err(RipRipError::Bug("Short read during CSW status phase."));
        }

        let csw = CommandStatusWrapper::from_bytes(&csw_raw);

        // Verify protocol sync state against our local tag.
        if csw.signature != CSW_SIGNATURE || csw.tag != current_tag {
            return Err(RipRipError::Bug(
                "Fatal Protocol Desync: CSW validation error.",
            ));
        }

        match csw.status {
            0 => Ok(transferred),
            1 => Err(RipRipError::CdRead),
            2 => Err(RipRipError::Bug("USB BOT phase error.")),
            _ => Err(RipRipError::Bug("Illegal status code.")),
        }
    }

    /// # Check Disc Mode.
    ///
    /// This makes sure an audio CD is actually present in the drive.
    ///
    /// ## Errors
    ///
    /// Returns an error if the disc is missing or unsupported.
    fn check_disc_mode__(&self) -> Result<(), RipRipError> {
        // Worst-case: 4 bytes (header) + (99 tracks * 8 bytes) + 8 bytes (Lead-out).
        const ALLOC_LEN: u16 = 4 + 99 * 8 + 8;

        let mut cmd = [0u8; 10];
        cmd[0] = mmc::READ_TOC;
        cmd[1] = 0x00; // 0x00 = Native LBA Format.
        cmd[2] = mmc::TOC_FORMAT_TOC; // Format 0: Standard Table of Contents.
        cmd[6] = mmc::FIRST_TRACK; // Start reading starting from Track 1.

        cmd[7..9].copy_from_slice(&ALLOC_LEN.to_be_bytes());

        let mut buf = vec![0u8; ALLOC_LEN as usize];
        self.exec_scsi_read(&cmd, &mut buf)
            .or(Err(RipRipError::DiscMode))?;

        let first_track = buf[2];
        let last_track = buf[3];

        // Sanity check.
        if last_track == 0 || first_track > last_track {
            return Err(RipRipError::DiscMode);
        }

        // Search the descriptors. If an audio track is found, early exit,
        // otherwise default to a DiscMode error.
        buf[4..]
            .chunks_exact(8)
            .take((last_track - first_track + 1) as usize)
            .any(|desc| (desc[1] & mmc::CTRL_DATA_TRACK) == 0)
            .then_some(())
            .ok_or(RipRipError::DiscMode)
    }

    fn read_cdtext(&self) -> Option<Vec<u8>> {
        const TOC_LEN: u16 = 2048;

        let mut cmd = [0u8; 10];
        cmd[0] = mmc::READ_TOC;
        cmd[2] = mmc::TOC_FORMAT_CDTEXT;
        cmd[6] = 0x00; // Track number to start reading from (0 = entire disc).

        let alloc_len = TOC_LEN;
        cmd[7..9].copy_from_slice(&alloc_len.to_be_bytes());

        let mut buf = vec![0u8; TOC_LEN as usize];
        self.exec_scsi_read(&cmd, &mut buf).ok()?;

        let len = u16::from_be_bytes([buf[0], buf[1]]);
        if len == 0 {
            return None; // No CD-Text exists on this disc.
        }

        // Commands like READ_TOC return a 2-byte header containing the data length.
        // However, this length field excludes the 2 bytes of the length field itself.
        let total_valid_bytes = (len + 2) as usize;
        let truncate_len = std::cmp::min(total_valid_bytes, buf.len());
        buf.truncate(truncate_len);

        Some(buf)
    }

    fn get_toc_header(&self) -> Result<(u8, u8), RipRipError> {
        let mut cmd = [0u8; 12];
        cmd[0] = mmc::READ_TOC;
        cmd[1] = 0x00; // 0x00 = Native LBA Format
        cmd[2] = mmc::TOC_FORMAT_TOC; // Format 0: Standard Table of Contents
        cmd[6] = mmc::FIRST_TRACK; // Start reading starting from Track 1.

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

impl<C: UsbContext> Cdda for LibusbInstance<C> {
    fn first_track_num(&self) -> Result<u8, RipRipError> {
        let (first, _) = self.get_toc_header()?;

        if first == 0 {
            Err(RipRipError::FirstTrackNum)
        } else {
            Ok(first)
        }
    }

    fn leadout_lba(&self) -> Result<u32, RipRipError> {
        // In the SCSI MMC specification, the leadout track information is
        // explicitly queried using the standard magic track index 0xAA.
        self.track_lba_start(mmc::LEAD_OUT)
    }

    fn num_tracks(&self) -> Result<u8, RipRipError> {
        let (first, last) = self.get_toc_header()?;

        if last == 0 {
            Err(RipRipError::NumTracks)
        } else {
            // Handles discs that might not explicitly start at track 1
            Ok(last - first + 1)
        }
    }

    fn track_format(&self, idx: u8) -> Result<bool, RipRipError> {
        let (control_adr, _) = self
            .get_track_descriptor(idx)
            .map_err(|_| RipRipError::TrackFormat(idx))?;

        // In SCSI MMC TOC structures, the 4-bit CONTROL field dictates data types.
        // Bit 2 (0x04) is set if the track is a data track, and clear if it's audio.
        let is_data = (control_adr & mmc::CTRL_DATA_TRACK) > 0;

        Ok(!is_data)
    }

    fn track_lba_start(&self, idx: u8) -> Result<u32, RipRipError> {
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

    fn cdtext(&self, idx: u8, kind: CDTextKind) -> Option<String> {
        if let Some(metadata) = &self.metadata {
            return metadata.layers[0]
                .catalog
                .get(&(kind.into(), idx))
                .map(|s| s.clone());
        }
        None
    }

    // /// # Track ISRC.
    // ///
    // /// Fetches the International Standard Recording Code directly from the sub-Q channel
    // /// using SCSI opcode 0x42 (READ SUB-CHANNEL).
    // pub(super) fn track_isrc(&self, idx: u8) -> Option<String> {
    //     let mut cmd = [0u8; 12];
    //     cmd[0] = mmc::READ_SUB_CHANNEL;
    //     cmd[1] = 0x02; // MSF Address Mode format flag (Bit 1 set)
    //     cmd[2] = 0x40; // Sub-Q Channel Data Enable (Bit 6 set)
    //     cmd[3] = mmc::SUB_FORMAT_ISRC; // Data Format: 0x03 (International Standard Recording Code)
    //     cmd[6] = idx; // Target Track index parameter

    //     cmd[8] = 24; // Allocation Length: 24 Bytes allocation footprint

    //     let mut buf = [0u8; 24];
    //     self.exec_scsi_read(&cmd, &mut buf).ok()?;

    //     // Verify sub-channel execution parameters
    //     let data_format = buf[3];
    //     let subq_element_valid = buf[4]; // Sub-Q channel data status indicator flag

    //     // If the drive confirms sub-Q tracking sync data exists
    //     if data_format == mmc::SUB_FORMAT_ISRC && subq_element_valid == 0x01 {
    //         let is_isrc_valid = (buf[12] & 0x80) != 0; // Bit 7 maps existence state
    //         if is_isrc_valid {
    //             // Raw string slice extraction out of the fixed-offset 12-byte block
    //             let raw_ascii = &buf[13..25];
    //             return String::from_utf8(raw_ascii.to_vec())
    //                 .ok()
    //                 .map(|s| s.trim().to_string());
    //         }
    //     }
    //     None
    // }

    fn mcn(&self) -> Option<Barcode> {
        if let Some(barcode_str) = self.cdtext(0, CDTextKind::Barcode) {
            if let Ok(barcode) = Barcode::try_from(barcode_str.as_bytes()) {
                return Some(barcode);
            }
        }

        self.mcn__()
    }

    fn drive_vendor_model(&self) -> Option<DriveVendorModel> {
        let mut cmd = [0u8; 12];
        cmd[0] = mmc::spc::INQUIRY;
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

    fn is_sector_bad(&self, lsn: i32) -> bool {
        SHITLIST.with_borrow(|q| q.contains(&lsn))
    }

    fn read_cd(
        &self,
        buf: &mut [u8],
        lsn: i32,
        c2: bool,
        sub: u8,
        _block_size: u16,
    ) -> Result<(), RipRipError> {
        let mut cmd = [0u8; 12];
        cmd[0] = mmc::READ_CD;
        cmd[1] = 0x04; // Expected Sector Type field flag -> 0x04 means CD-DA Audio

        // riprip's addressing parameters are already absolute LBAs.
        let lba = lsn as u32;
        cmd[2..6].copy_from_slice(&lba.to_be_bytes());

        // Transfer exactly 1 sector at a time
        cmd[6..9].copy_from_slice(&1u32.to_be_bytes()[1..4]);

        // Byte 9 is the Selection Field flag byte:
        // Bit 4: User Data Selection (Set to 1 to read the 2352 bytes audio payload)
        // Bit 2..1: C2 Error Flag selection allocation (10b means include 294 bytes C2 space)
        let user_data_flag = 0x10;
        let c2_flag = if c2 { 0x02 } else { 0x00 };
        cmd[9] = user_data_flag | c2_flag;

        // Byte 10 defines the Sub-channel Selection configuration flags:
        // 0x00 = No sub-channel data requested
        // 0x02 = Raw Subchannel Data payload (16 bytes payload space)
        cmd[10] = sub;

        // Dispatch via your battle-tested SCSI core runner wrapper
        match self.exec_scsi_read(&cmd, buf) {
            Ok(_) => Ok(()),
            Err(_) => {
                SHITLIST.with(|q| q.borrow_mut().insert(lsn));
                Err(RipRipError::CdRead)
            }
        }
    }
}

impl<C: UsbContext> LibusbInstance<C> {
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
