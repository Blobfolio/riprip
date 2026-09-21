/*!
# Rip Rip Hooray: `libusb` Wrappers

Somewhat useful documentation:
<https://docs.rs/rusb/0.9.4/rusb/>
*/

mod bot;
mod device;
mod mmc;

use crate::{
	CddaDriverNewExt,
	RipRipError,
	macros::log,
};
use mmc::{
	MmcDriverExt,
	TransportExt,
};
use nix::unistd::{
	Uid,
	setuid,
};
use rusb::{
	Device,
	DeviceHandle,
	DeviceList,
	Direction,
	GlobalContext,
	TransferType,
	UsbContext,
};
use std::{
	env,
	sync::atomic::{
		AtomicU32,
		Ordering,
	},
	path::Path,
	time::Duration,
};



/// # Write Bulk Timeout.
const WRITE_BULK_TIMEOUT: Duration = Duration::from_secs(2);

/// # Read Bulk Timeout.
const READ_BULK_TIMEOUT: Duration = Duration::from_secs(5);

/// # Status Read Timeout.
const STATUS_READ_TIMEOUT: Duration = Duration::from_secs(2);



/// # Find and Open Device.
fn find_and_open_device<C: UsbContext>(devices: &DeviceList<C>, vid: u16, pid: u16)
-> Result<DeviceHandle<C>, RipRipError> {
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
			}
			else { None }
		})
		.unwrap_or(Err(RipRipError::DeviceOpen(Some(format!("{pid}:{vid}")))))
}

/// # Find and Open CD Drive.
fn find_and_open_cd_drive<C: UsbContext>(devices: &DeviceList<C>)
-> Result<DeviceHandle<C>, RipRipError> {
	devices
		.iter()
		.find_map(|device| {
			let config_desc = device.active_config_descriptor().ok()?;

			let is_optical = config_desc.interfaces().any(|interface|
				interface.descriptors().any(|desc|
					desc.class_code() == bot::CLASS_MASS_STORAGE &&
					bot::OpticalDriveSubclass::from_u8(desc.sub_class_code()).is_some() &&
					desc.protocol_code() == bot::PROTOCOL_BULK_ONLY
				)
			);

			if is_optical {
				log!(@debug "Found optical drive ({:?}).", device);
				Some(
					device
						.open()
						.map_err(|e| RipRipError::DeviceOpen(Some(e.to_string()))),
				)
			}
			else { None }
		})
		.unwrap_or(Err(RipRipError::DeviceOpen(None)))
}

/// # Detect Bulk Endpoints.
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

	log!(
		@trace
		"Endpoints detected (in: {:#04x}, out: {:#04x}).",
		endpoints.bulk_in,
		endpoints.bulk_out,
	);

	Ok(endpoints)
}

#[derive(Debug, Default)]
/// # Bulk Endpoints.
struct Endpoints {
	/// # Input.
	bulk_in: u8,

	/// # Output.
	bulk_out: u8,
}

/// # USB Instance.
///
/// Pretty much all CD-related communications run through a single `LibusbInstance`
/// object.
pub(crate) struct LibusbInstance<C: UsbContext = GlobalContext> {
	/// # USB Device.
	device_handle: DeviceHandle<C>,

	/// # Interface ID.
	interface_id: u8,

	/// # Bulk Endpoints.
	endpoints: Endpoints,

	/// # Command Block Wrapper Tag.
	cbw_tag: AtomicU32,
}

impl<C: UsbContext> Drop for LibusbInstance<C> {
	fn drop(&mut self) {
		if let Err(e) = self.device_handle.release_interface(self.interface_id) {
			log!(@error "Releasing USB interface {}: {}.", self.interface_id, e);
		}

		if let Err(e) = self.device_handle.attach_kernel_driver(self.interface_id) {
			log!(@error "Reattaching kernel driver for interface {}: {}.", self.interface_id, e);
		}
	}
}

impl<C: UsbContext> LibusbInstance<C> {
	/// # With Context.
	///
	/// Probe the device for more specific information about itself and the
	/// loaded disc, if any, returning `Self` (with those details) if
	/// successful.
	pub(super) fn with_context<P>(context: &C, dev: Option<P>)
	-> Result<Self, RipRipError>
	where P: AsRef<Path> {
		log!(@debug "Initializing `libusb` driver.");

		let devices = context
			.devices()
			.map_err(|e| RipRipError::DeviceOpen(Some(e.to_string())))?;

		let device_handle = if let Some(path) = dev {
			let device_path = path.as_ref().to_string_lossy();

			log!(@debug "Device path {device_path}.");

			if let Some((vid, pid)) = device::get_desc(&path)? {
				log!(@debug "Found USB device (ID {vid:04x}:{pid:04x}).");
				find_and_open_device(&devices, vid, pid)?
			}
			else {
				return Err(RipRipError::DeviceOpen(Some(device_path.into_owned())));
			}
		}
		else { find_and_open_cd_drive(&devices)? };

		let endpoints = detect_bulk_endpoints(&device_handle.device())?;
		let interface_id = 0;

		// Check if kernel driver is owning our device and detach it if so.
		if device_handle.kernel_driver_active(interface_id) == Ok(true) {
			device_handle
				.detach_kernel_driver(interface_id)
				.map_err(|e| RipRipError::Device(e.to_string()))?;
		}

		// Claim it!
		device_handle
			.claim_interface(interface_id)
			.map_err(|_| RipRipError::Bug("Failed to claim iface."))?;

		// SUDO isn't needed anymore; try to reset to the original user.
		if let Ok(sudo_uid) = env::var("SUDO_UID") {
			let original_uid: u32 = sudo_uid
				.parse()
				.map_err(|_| RipRipError::Bug("SUDO_UID is not a valid integer."))?;

			setuid(Uid::from_raw(original_uid))
				.map_err(|_| RipRipError::Bug("Failed to drop process privileges."))?;
		}

		let out = Self {
			device_handle,
			interface_id,
			endpoints,
			cbw_tag: AtomicU32::default(),
		};

		out.check_disc_mode__()?;
		out.check_c2__()?;

		Ok(out)
	}
}

impl<T: UsbContext> TransportExt for LibusbInstance<T> {
	/// # Submit.
	fn submit<const N: usize>(&self, cdb: &[u8; N], buf: &mut [u8])
	-> Result<usize, RipRipError> {
		use bot::{
			CSW_LEN,
			CommandBlockWrapper,
			CommandStatusWrapper,
		};

		const {
			assert!(N != 0 && N <= 16, "BUG: CDB must have length of 1..=16.");
		}

		// Read and increment the local counter attached directly to this specific drive.
		let current_tag = self.cbw_tag.fetch_add(1, Ordering::Relaxed);
		let data_len = buf.len();

		let cbw = CommandBlockWrapper::new(
			current_tag,
			u32::try_from(buf.len()).map_err(|e| RipRipError::Internal(e.to_string()))?,
			0x80, // Device-to-Host
			0,
			cdb,
		);

		let cbw_bytes = cbw.to_bytes();
		self.device_handle
			.write_bulk(self.endpoints.bulk_out, &cbw_bytes, WRITE_BULK_TIMEOUT)
			.map_err(|e| {
				log!(@trace [cbw, buf] "CBW write failed.");
				RipRipError::Internal(e.to_string())
			})?;

		// Skip the read phase entirely if no data transfer is expected.
		let transferred = if data_len > 0 {
			match self
				.device_handle
				.read_bulk(self.endpoints.bulk_in, buf, READ_BULK_TIMEOUT)
			{
				Ok(n) => n,
				Err(rusb::Error::Pipe) => {
					log!(@trace [cbw, buf] "CBW read pipe failed.");
					self.device_handle
						.clear_halt(self.endpoints.bulk_in)
						.map_err(|e| RipRipError::Internal(e.to_string()))?;
					0
				}
				Err(e) => {
					log!(@trace [cbw, buf] "CBW read failed.");
					return Err(RipRipError::Internal(e.to_string()))
				},
			}
		}
		else { 0 };

		let mut csw_raw = [0_u8; CSW_LEN];
		let len = self
			.device_handle
			.read_bulk(self.endpoints.bulk_in, &mut csw_raw, STATUS_READ_TIMEOUT)
			.map_err(|e| {
				log!(@trace [cbw, buf, transferred] "CSW read failed.");
				RipRipError::Internal(e.to_string())
			})?;

		if len != CSW_LEN {
			return Err(RipRipError::Bug("Short read during CSW status phase."));
		}

		let csw = CommandStatusWrapper::from_bytes(&csw_raw);

		// Verify protocol sync state against our local tag.
		if ! csw.is_valid(current_tag) {
			log!(@trace [cbw, buf, transferred, csw] "Invalid CSW.");
			return Err(RipRipError::Bug(
				"Fatal Protocol Desync: CSW validation error.",
			));
		}

		if csw.status() == 0 {
			return Ok(transferred);
		}

		log!(@trace [cbw, buf, transferred, csw] "CSW did not pass.");
		match csw.status() {
			1 => Err(RipRipError::CdRead),
			2 => Err(RipRipError::Bug("USB BOT phase error.")),
			_ => Err(RipRipError::Bug("Illegal status code.")),
		}
	}
}

impl CddaDriverNewExt for LibusbInstance<GlobalContext> {
	/// # New Instance.
	fn new<P>(dev: Option<P>) -> Result<Self, RipRipError>
	where P: AsRef<Path> {
		Self::with_context(&GlobalContext::default(), dev)
	}
}
