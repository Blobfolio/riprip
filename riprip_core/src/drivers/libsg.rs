/*!
# Rip Rip Hooray: `libsg` Wrappers

Somewhat useful documentation:
<https://www.kernel.org/doc/html/latest/scsi/scsi-generic.html>
*/

use crate::{Barcode, CD_LEADIN, CDTextKind, CddaDriverExt, DriveVendorModel, RipRipError};

use crate::drivers::cdtext;
use crate::drivers::mmc::{self, Drive, Transport};

use libc;

use std::{fs::OpenOptions, fs::File, os::fd::AsRawFd};
use std::path::{Path, PathBuf};

const SG_IO: libc::c_ulong = 0x2285;
const SG_DXFER_FROM_DEV: i32 = -3;
const DEFAULT_PATH: &str = "/dev/sr0";

#[repr(C)]
struct SgIoHdr {
    interface_id: i32,
    dxfer_direction: i32,
    cmd_len: u8,
    mx_sb_len: u8,
    iovec_count: u16,
    dxfer_len: u32,
    dxferp: *mut libc::c_void,
    cmdp: *mut u8,
    sbp: *mut u8,
    timeout: u32,
    flags: u32,
    pack_id: i32,
    usr_ptr: *mut libc::c_void,
    status: u8,
    masked_status: u8,
    msg_status: u8,
    sb_len_wr: u8,
    host_status: u16,
    driver_status: u16,
    resid: i32,
    duration: u32,
    info: u32,
}

/// # /dev/sg Instance.
///
/// Pretty much all CD-related communications run through a single `LibsgInstance`
/// object.
pub(crate) struct LibsgInstance {
    /// # Device file.
    device: File,

    /// # CD-Text.
    metadata: Option<cdtext::Metadata>,
}

impl Transport for LibsgInstance {
    #[expect(unsafe_code, reason = "For FFI.")]
    fn submit<const N: usize>(&self, cdb: &[u8; N], buf: &mut [u8]) -> Result<usize, RipRipError> {
        let mut sense = [0u8; 32];

        let mut hdr = SgIoHdr {
            interface_id: b'S' as i32,
            dxfer_direction: SG_DXFER_FROM_DEV,
            cmd_len: cdb.len() as u8,
            mx_sb_len: sense.len() as u8,
            iovec_count: 0,
            dxfer_len: buf.len() as u32,
            dxferp: buf.as_mut_ptr() as *mut _,
            cmdp: cdb.as_ptr() as *mut u8,
            sbp: sense.as_mut_ptr(),
            timeout: 10_000, // in milliseconds
            flags: 0,
            pack_id: 0,
            usr_ptr: std::ptr::null_mut(),
            status: 0,
            masked_status: 0,
            msg_status: 0,
            sb_len_wr: 0,
            host_status: 0,
            driver_status: 0,
            resid: 0,
            duration: 0,
            info: 0,
        };

        let ret = unsafe { libc::ioctl(self.device.as_raw_fd(), SG_IO, &mut hdr) };

        if ret < 0 {
            return Err(RipRipError::Internal(
                std::io::Error::last_os_error().to_string(),
            ));
        }

        // eprintln!("SCSI status: 0x{:02x}", hdr.status);

        if hdr.sb_len_wr != 0 {
            eprintln!("Sense: {:02x?}", &sense[..hdr.sb_len_wr as usize]);
        }

        // Number of bytes actually transferred.
        Ok(buf.len() - hdr.resid as usize)
    }
}

impl Drive for LibsgInstance {}

impl CddaDriverExt for LibsgInstance {
    /// # New!
    fn new<P>(dev: Option<P>) -> Result<Self, RipRipError>
    where
        P: AsRef<Path>,
    {
        let path = dev
            .map(|p| p.as_ref().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(DEFAULT_PATH));

        let device = OpenOptions::new()
            .read(true)
            .write(false)
            .open(&path)
            .map_err(|e| RipRipError::DeviceOpen(Some(format!("{}: {e}", path.display()))))?;

        let mut out = Self {
            device,
            metadata: None,
        };

        out.check_disc_mode__()?;

        out.check_c2__()?;

        if let Some(buf) = out.read_cdtext()? {
            let opt = cdtext::Metadata::from_bytes(&buf).map_err(|_| RipRipError::CdText)?;
            if let Some(metadata) = opt {
                out.metadata.replace(metadata);
            }
        }
        // dbg!(&out.metadata);

        Ok(out)
    }

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

    /// # Track LBA Start.
    fn track_lba_start(&self, idx: u8) -> Result<u32, RipRipError> {
        if idx == 0 {
            return Err(RipRipError::TrackNumber(0));
        }

        let (_, lba) = self
            .get_track_descriptor(idx)
            .map_err(|_| RipRipError::TrackLba(idx))?;

        Ok(lba + u32::from(CD_LEADIN))
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

        None
    }

    fn mcn_subchannel(&self) -> Option<Barcode> {
        self.mcn_subchannel__().ok().flatten()
    }

    fn drive_vendor_model(&self) -> Option<DriveVendorModel> {
        self.drive_vendor_model__()
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
                super::set_bad_sector(lsn);
                Err(RipRipError::CdRead)
            }
        }
    }
}
