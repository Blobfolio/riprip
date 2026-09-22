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
	path::{
		Path,
		PathBuf,
	},
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
					device.open().map_err(|e| {
						log!(@trace [vid, pid] "{e}");
						RipRipError::Internal("Device open", e.to_string())
					}),
				)
			}
			else { None }
		})
		.unwrap_or_else(|| Err(RipRipError::DeviceOpen(Some(format!("{pid}:{vid}")))))
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
					device.open().map_err(|e| {
						log!(@trace [device] "{e}");
						RipRipError::Internal("Device open", e.to_string())
					}),
				)
			}
			else { None }
		})
		.unwrap_or_else(|| Err(RipRipError::DeviceOpen(None)))
}

/// # Detect Bulk Endpoints.
fn detect_bulk_endpoints<T: UsbContext>(device: &Device<T>) -> Option<Endpoints> {
	let config_desc = match device.active_config_descriptor() {
		Ok(v) => v,
		Err(err) => {
			log!(
				@trace
				[err]
				"Failed to get active configuration descriptor for device.",
			);
			return None;
		},
	};

	let (bulk_in, bulk_out) = config_desc
		.interfaces()
		.flat_map(|iface| iface.descriptors())
		.filter(|iface_desc| iface_desc.class_code() == bot::CLASS_MASS_STORAGE)
		.flat_map(|iface_desc| iface_desc.endpoint_descriptors())
		.filter(|ep_desc| ep_desc.transfer_type() == TransferType::Bulk)
		.fold((None, None), |acc, ep| match ep.direction() {
			Direction::In => (Some(ep.address()), acc.1),
			Direction::Out => (acc.0, Some(ep.address())),
		});

	// Return 'em if we got 'em.
	if let Some(bulk_in) = bulk_in && let Some(bulk_out) = bulk_out {
		log!(
			@trace
			"Endpoints detected (in: {:#04x}, out: {:#04x}).",
			bulk_in,
			bulk_out,
		);

		Some(Endpoints { bulk_in, bulk_out })
	}
	// Boo.
	else {
		log!(@trace [bulk_in, bulk_out] "Unable to determine bulk endpoint(s).");
		None
	}

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
		// Generic Error.
		fn err(dev: Option<&Path>) -> RipRipError {
			RipRipError::DeviceOpen(dev.map(|v| v.to_string_lossy().into_owned()))
		}

		// AsRef gets really fucking annoying. Haha.
		let dev: Option<PathBuf> = dev.map(|v| v.as_ref().to_path_buf());

		log!(@debug "Initializing `libusb` driver.");

		let devices = context.devices()
			.map_err(|e| {
				log!(@trace [dev] "{e}");
				err(dev.as_deref())
			})?;

		let device_handle =
			if let Option::<&Path>::Some(path) = dev.as_deref() {
				log!(@debug "Device path {}.", path.display());

				if let Some((vid, pid)) = device::get_desc(&path)? {
					log!(@debug "Found USB device (ID {vid:04x}:{pid:04x}).");
					find_and_open_device(&devices, vid, pid)?
				}
				else { return Err(err(dev.as_deref())); }
			}
			else {
				find_and_open_cd_drive(&devices)?
			};

		let endpoints = detect_bulk_endpoints(&device_handle.device())
			.ok_or_else(|| err(dev.as_deref()))?;
		let interface_id = 0;

		// Check if kernel driver is owning our device and detach it if so.
		if device_handle.kernel_driver_active(interface_id) == Ok(true) {
			device_handle
				.detach_kernel_driver(interface_id)
				.map_err(|e| {
					log!(@trace [dev] "{e}");
					err(dev.as_deref())
				})?;
		}

		// Claim it!
		device_handle
			.claim_interface(interface_id)
			.map_err(|e| {
				log!(@trace [dev, interface_id] "{e}");
				RipRipError::Bug("Failed to claim iface.")
			})?;

		// SUDO isn't needed anymore; try to reset to the original user.
		if let Ok(sudo_uid) = env::var("SUDO_UID") {
			let original_user_id: u32 = sudo_uid.parse()
				.map_err(|_| {
					log!(@trace [sudo_uid] "Unable to parse SUDO_UID.");
					RipRipError::Bug("SUDO_UID is not a valid integer.")
				})?;

			let user_id = Uid::from_raw(original_user_id);
			if ! user_id.is_root() {
				setuid(user_id)
					.map_err(|e| {
						log!(@trace [original_user_id] "{e}");
						RipRipError::Bug("Failed to drop process privileges.")
					})?;

				log!(@debug "Dropped sudo privileges.");
			}
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
	fn submit<const N: usize>(&self, cdb: &[u8; N], buf: &mut [u8], ctx: &'static str)
	-> Result<usize, RipRipError> {
		use bot::{
			CSW_LEN,
			CommandBlockWrapper,
			CommandStatusWrapper,
		};

		const {
			assert!(
				N == 6 || N == 10 || N == 12,
				"BUG: CDB must have length of 6, 10, or 12.",
			);
		}

		// Read and increment the local counter attached directly to this specific drive.
		let current_tag = self.cbw_tag.fetch_add(1, Ordering::Relaxed);
		let data_len = buf.len();

		let cbw = CommandBlockWrapper::new(
			current_tag,
			u32::try_from(buf.len())
				.map_err(|_| {
					log!(@trace [ctx, buf.len()] "Command block buffer exceeds u32::MAX.");
					RipRipError::Bug("Command block buffer exceeds u32::MAX.")
				})?,
			0x80, // Device-to-Host
			0,
			cdb,
		);

		let cbw_bytes = cbw.to_bytes();
		self.device_handle
			.write_bulk(self.endpoints.bulk_out, &cbw_bytes, WRITE_BULK_TIMEOUT)
			.map_err(|e| {
				log!(@trace [ctx, cbw, buf] "Command block write failed.");
				RipRipError::Internal("Command block write", e.to_string())
			})?;

		// Skip the read phase entirely if no data transfer is expected.
		let transferred =
			if data_len == 0 { 0 }
			else {
				match self
					.device_handle
					.read_bulk(self.endpoints.bulk_in, buf, READ_BULK_TIMEOUT)
				{
					Ok(n) => n,
					Err(rusb::Error::Pipe) => {
						log!(@trace [ctx, cbw, buf] "Command block read pipe failed.");
						self.device_handle
							.clear_halt(self.endpoints.bulk_in)
							.map_err(|e| RipRipError::Internal("Command block clear", e.to_string()))?;
						0
					}
					Err(e) => {
						log!(@trace [ctx, cbw, buf] "Command block read failed.");
						return Err(RipRipError::Internal("Command block read", e.to_string()))
					},
				}
			};

		let mut csw_raw = [0_u8; CSW_LEN];
		let len = self
			.device_handle
			.read_bulk(self.endpoints.bulk_in, &mut csw_raw, STATUS_READ_TIMEOUT)
			.map_err(|e| {
				log!(@trace [ctx, cbw, buf, transferred] "Command status read failed.");
				RipRipError::Internal("Command status read", e.to_string())
			})?;

		if len != CSW_LEN {
			log!(
				@trace
				[ctx, CSW_LEN, len, cbw, buf]
				"Short read during CSW status phase.",
			);
			return Err(RipRipError::Bug("Short read during CSW status phase."));
		}

		let csw = CommandStatusWrapper::from_bytes(&csw_raw);

		// Verify protocol sync state against our local tag.
		if ! csw.is_valid(current_tag) {
			log!(
				@trace
				[ctx, cbw, buf, transferred, csw]
				"Fatal Protocol Desync: CSW validation error.",
			);
			return Err(RipRipError::Bug(
				"Fatal Protocol Desync: CSW validation error.",
			));
		}

		// Happy!
		if csw.status() == 0 { return Ok(transferred); }

		log!(
			@trace
			[ctx, cbw, buf, transferred, csw]
			"Command status ({}) did not pass.",
			csw.status(),
		);
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
