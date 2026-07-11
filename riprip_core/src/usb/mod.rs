/*!
# Rip Rip Hooray: `libusb` Wrappers

Somewhat useful documentation:
<https://docs.rs/rusb/0.9.4/rusb/>
*/

mod bot;
mod cdtext;
mod device;
mod mmc;

use crate::{
    Barcode, CDTextKind, Cdda, DriveVendorModel, KillSwitch, RipRipError, CD_DATA_C2_SIZE,
    CD_DATA_SIZE, CD_DATA_SUBCHANNEL_SIZE, CD_LEADIN,
};

use crate::usb::mmc::{Drive, Transport};

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
    pub(super) fn with_context<P>(context: C, dev: Option<P>) -> Result<Self, RipRipError>
    where
        P: AsRef<Path>,
    {
        let devices = context
            .devices()
            .map_err(|e| RipRipError::DeviceOpen(Some(e.to_string())))?;

        let device_handle = if let Some(path) = dev {
            if let Some((vid, pid)) = device::get_desc(&path)? {
                // eprintln!("{vid:04x}:{pid:04x}");
                find_and_open_device(devices, vid, pid)?
            } else {
                let path_str = path.as_ref().to_string_lossy().to_string();
                return Err(RipRipError::DeviceOpen(Some(path_str)));
            }
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
    /// device path is obviously wrong.
    pub(super) fn new_global<P>(dev: Option<P>) -> Result<Self, RipRipError>
    where
        P: AsRef<Path>,
    {
        Self::with_context(GlobalContext::default(), dev)
    }
}

impl<T: UsbContext> Transport for LibusbInstance<T> {
    fn submit<const N: usize>(&self, cdb: &[u8; N], buf: &mut [u8]) -> Result<usize, RipRipError> {
        use bot::{
            CommandBlockWrapper, CommandStatusWrapper, CBW_SIGNATURE, CSW_LEN, CSW_SIGNATURE,
        };

        const { assert!(N <= 16, "CDB cannot exceed 16 bytes.") };

        // Read and increment the local counter attached directly to this specific drive.
        let current_tag = self.cbw_tag.fetch_add(1, Ordering::Relaxed);
        let data_len = buf.len();

        let mut cbw = CommandBlockWrapper {
            signature: CBW_SIGNATURE,
            tag: current_tag,
            data_transfer_length: data_len as u32,
            flags: 0x80, // Device-to-Host
            lun: 0,
            cb_length: N as u8,
            cdb: [0u8; 16],
        };
        cbw.cdb[..N].copy_from_slice(cdb);

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
        if !csw.is_valid(current_tag) {
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
}

// Let's Rock.
impl<T: UsbContext> Drive for LibusbInstance<T> {}

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

    fn mcn(&self) -> Option<Barcode> {
        if let Some(barcode_str) = self.cdtext(0, CDTextKind::Barcode) {
            if let Ok(barcode) = Barcode::try_from(barcode_str.as_bytes()) {
                return Some(barcode);
            }
        }

        self.mcn__()
    }

    fn drive_vendor_model(&self) -> Option<DriveVendorModel> {
        self.drive_vendor_model__()
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
        match self.read_cd__(buf, lsn, c2, sub) {
            Ok(_) => Ok(()),
            Err(_) => {
                SHITLIST.with(|q| q.borrow_mut().insert(lsn));
                Err(RipRipError::CdRead)
            }
        }
    }
}
