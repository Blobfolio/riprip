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
use dactyl::{
	NiceElapsed,
	NiceU32,
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
	EndpointDescriptor,
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



/// # Open Retry Timeout.
const OPEN_RETRY_TIMEOUT: Duration = Duration::from_secs(2);

/// # Write Bulk Timeout.
const WRITE_BULK_TIMEOUT: Duration = Duration::from_secs(2);

/// # Read Bulk Timeout.
const READ_BULK_TIMEOUT: Duration = Duration::from_secs(5);

/// # Status Read Timeout.
const STATUS_READ_TIMEOUT: Duration = Duration::from_secs(2);



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
		// AsRef gets really fucking annoying. Haha.
		let dev: Option<PathBuf> = dev.map(|v| v.as_ref().to_path_buf());

		log!(@debug "Initializing `libusb` driver.");

		// Find and open the device.
		let devices = context.devices().map_err(|e| {
			log!(@trace [dev] "{e}");
			open_err(dev.as_deref())
		})?;
		let device = find_device(&devices, dev.as_deref())?;
		log!(@debug "Found optical drive ({device:?}).");
		let device_handle = open_device(&device, dev.as_deref())?;
		let endpoints = Endpoints::from_device(&device_handle.device())
			.ok_or_else(|| open_err(dev.as_deref()))?;

		// Check if kernel driver is owning our device and detach it if so.
		let interface_id = 0;
		if device_handle.kernel_driver_active(interface_id) == Ok(true) {
			device_handle.detach_kernel_driver(interface_id).map_err(|e| {
				log!(@trace [dev] "{e}");
				open_err(dev.as_deref())
			})?;
		}

		// Claim it!
		device_handle.claim_interface(interface_id).map_err(|e| {
			log!(@trace [dev, interface_id] "{e}");
			RipRipError::Bug("Failed to claim iface.")
		})?;

		// SUDO isn't needed anymore; try to reset to the original user.
		if let Ok(sudo_uid) = env::var("SUDO_UID") {
			let original_user_id: u32 = sudo_uid.parse().map_err(|_| {
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

		// We have enough for an instance.
		let out = Self {
			device_handle,
			interface_id,
			endpoints,
			cbw_tag: AtomicU32::default(),
		};

		// Double check the disc mode and C2 support.
		out.check_disc_mode__()?;
		out.check_c2__()?;

		// Done!
		Ok(out)
	}
}

impl<T: UsbContext> TransportExt for LibusbInstance<T> {
	/// # Submit.
	fn submit<const N: usize>(&self, cdb: &[u8; N], buf: &mut [u8], ctx: &'static str)
	-> Result<usize, RipRipError> {
		use bot::{
			CommandBlockWrapper,
			CommandStatusWrapper,
			CSW_LEN,
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
				@trace [ctx, CSW_LEN, len, cbw, buf]
				"Short read during CSW status phase.",
			);
			return Err(RipRipError::Bug("Short read during CSW status phase."));
		}

		let csw = CommandStatusWrapper::from_bytes(&csw_raw);

		// Verify protocol sync state against our local tag.
		if ! csw.is_valid(current_tag) {
			log!(
				@trace [ctx, cbw, buf, transferred, csw]
				"Fatal Protocol Desync: CSW validation error.",
			);
			return Err(RipRipError::Bug(
				"Fatal Protocol Desync: CSW validation error.",
			));
		}

		// Expect residue to be zero.
		let residual = csw.data_residue();
		if residual != 0 {
			log!(
				@trace [ctx, csw, len, residual]
				"Drive reported {residual} residual bytes of data.",
				residual=NiceU32::from(residual),
			);
		}

		// Happy!
		if csw.status() == 0 { return Ok(transferred); }

		log!(
			@trace [ctx, cbw, buf, transferred, csw]
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



#[derive(Debug, Default)]
/// # Bulk Endpoints.
struct Endpoints {
	/// # Input.
	bulk_in: u8,

	/// # Output.
	bulk_out: u8,
}

impl Endpoints {
	#[must_use]
	/// # From Device.
	fn from_device<T: UsbContext>(device: &Device<T>) -> Option<Self> {
		let desc = match device.active_config_descriptor() {
			Ok(v) => v,
			Err(err) => {
				log!(
					@trace [err]
					"Failed to get active configuration descriptor for device.",
				);
				return None;
			},
		};

		// Digest and return the results of this terrible iterator!
		Self::from_descriptors(
			desc.interfaces()
				.flat_map(|iface| iface.descriptors())
				.filter(|iface_desc| iface_desc.class_code() == bot::CLASS_MASS_STORAGE)
				.flat_map(|iface_desc| iface_desc.endpoint_descriptors())
		)
	}

	#[must_use]
	/// # From Endpoint Descriptors.
	///
	/// Tease out the bulk input and output endpoints from the descriptor
	/// iterator, returning them if one of each is found, otherwise `None`.
	///
	/// This is actually pretty straightforward; the verbosity is merely for
	/// logging purposes.
	fn from_descriptors<'a, I: Iterator<Item=EndpointDescriptor<'a>>>(src: I) -> Option<Self> {
		let mut bulk_in = None;
		let mut bulk_out = None;

		for ep in src {
			match (ep.transfer_type(), ep.direction()) {
				(TransferType::Bulk, Direction::In) => {
					let input2 = ep.address();
					if let Some(input1) = bulk_in && input1 != input2 {
						log!(
							@trace [input1, input2]
							"Found conflicting bulk input endpoints.",
						);
						return None;
					}
					bulk_in.replace(input2);
				},
				(TransferType::Bulk, Direction::Out) => {
					let output2 = ep.address();
					if let Some(output1) = bulk_out && output1 != output2 {
						log!(
							@trace [output1, output2]
							"Found conflicting bulk output endpoints.",
						);
						return None;
					}
					bulk_out.replace(output2);
				},
				_ => {},
			}
		}

		match (bulk_in, bulk_out) {
			(Some(bulk_in), Some(bulk_out)) => {
				log!(
					@trace
					"Found bulk endpoints (in: {bulk_in:#04x}, out: {bulk_out:#04x}).",
				);
				Some(Self { bulk_in, bulk_out })
			},
			(None, Some(bulk_out)) => {
				log!(
					@trace [bulk_out]
					"Failed to find bulk input endpoint.",
				);
				None
			},
			(Some(bulk_in), None) => {
				log!(
					@trace [bulk_in]
					"Failed to find bulk output endpoint.",
				);
				None
			},
			(None, None) => {
				log!(@trace "Failed to find bulk endpoints.");
				None
			},
		}
	}
}



/// # Find Device.
///
/// Run through the list of USB devices, looking for one that matches the
/// provided path (if some), or the first that looks like an optical drive.
fn find_device<C: UsbContext>(devices: &DeviceList<C>, path: Option<&Path>)
-> Result<Device<C>, RipRipError> {
	// If we have a path, we'll want to match the specific video/product IDs.
	let ids: Option<(u16, u16)> =
		if let Some(p) = path {
			match device::get_desc(&p)? {
				Some(v) => Some(v),
				None => return Err(open_err(path)),
			}
		}
		else { None };

	// Loop the devices.
	for device in devices.iter() {
		if
			// It is an optical-ish drive and…
			let Ok(config_desc) = device.active_config_descriptor() &&
			config_desc.interfaces().any(|interface|
				interface.descriptors().any(|desc|
					desc.class_code() == bot::CLASS_MASS_STORAGE &&
					bot::OpticalDriveSubclass::from_u8(desc.sub_class_code()).is_some() &&
					desc.protocol_code() == bot::PROTOCOL_BULK_ONLY
				)
			) &&

			// We're looking for any device, or a specific one that matches.
			ids.is_none_or(|(vid, pid)|
				device.device_descriptor().is_ok_and(|desc|
					desc.vendor_id() == vid &&
					desc.product_id() == pid
				)
			)
		{
			return Ok(device);
		}
	}

	// No match.
	Err(open_err(path))
}

/// # Open Device.
fn open_device<C: UsbContext>(device: &Device<C>, path: Option<&Path>)
-> Result<DeviceHandle<C>, RipRipError> {
	use rusb::Error;

	let mut tries = 0;
	while tries < 3 {
		tries += 1;
		match device.open() {
			Ok(v) => return Ok(v),
			Err(Error::Busy) => {
				// Give it a second and try again.
				log!(
					@trace
					"Device is busy. Wait {} and retry.",
					NiceElapsed::from(OPEN_RETRY_TIMEOUT),
				);
				std::thread::sleep(OPEN_RETRY_TIMEOUT);
			},
			Err(Error::Access) => {
				log!(@error "Unable to open device: permission denied.");
				return Err(RipRipError::CdReadNotPermitted);
			},
			Err(Error::NotSupported) => {
				log!(@error "Unable to open device: operation not supported.");
				return Err(RipRipError::CdReadUnsupported);
			},
			Err(e) => {
				log!(@error "Unable to open device: {e}.");
				break;
			},
		}
	}

	Err(open_err(path))
}

/// # Open Error.
///
/// Most points of failure return a `DeviceOpen` error. This method handles the
/// somewhat tedious conversion of the path for use with that variant.
fn open_err(dev: Option<&Path>) -> RipRipError {
	RipRipError::DeviceOpen(dev.map(|v| v.to_string_lossy().into_owned()))
}
