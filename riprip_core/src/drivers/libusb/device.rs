/*!
# Rip Rip Hooray: USB Device

Provides cross-platform lookup to get a USB drive descriptor (Vendor and Product IDs)
from an OS device path.
*/

use crate::RipRipError;
use std::path::Path;



#[cfg(target_os = "macos")]
/// # Apple.
mod macos {
	use crate::log;
	use super::{
		Path,
		RipRipError,
	};
	use objc2_core_foundation::{
		CFDictionary,
		CFNumber,
		CFRetained,
		CFString,
		CFType,
		kCFAllocatorDefault,
	};
	use objc2_io_kit::{
		IOBSDNameMatching,
		IOIteratorNext,
		IOObjectRelease,
		IORegistryEntrySearchCFProperty,
		IOServiceGetMatchingServices,
		kIOMainPortDefault,
		kIORegistryIterateParents,
		kIORegistryIterateRecursively,
		kIOReturnSuccess,
		kIOServicePlane,
	};
	use std::{
		ffi::CString,
		os::unix::ffi::OsStrExt,
	};



	#[expect(unsafe_code, reason = "For FFI.")]
	#[must_use]
	/// # Get Numeric Property.
	///
	/// Query the media service for `key`, returning the code if found and
	/// valid.
	fn get_numeric_property(media_service: u32, key: &str) -> Option<u16> {
		let cf_key = CFString::from_str(key);

		// Safety: this is an FFI call.
		let prop = unsafe {
			#[expect(clippy::as_ptr_cast_mut, reason = "Unsatisfiable.")]
			IORegistryEntrySearchCFProperty(
				media_service,
				kIOServicePlane.as_ptr() as *mut _,
				Some(&cf_key),
				kCFAllocatorDefault,
				kIORegistryIterateRecursively | kIORegistryIterateParents,
			)
		};

		let cf_val = prop?;
		let number = CFType::downcast_ref::<CFNumber>(&cf_val)?;
		let val_i32 = number.as_i32()?;

		u16::try_from(val_i32).ok()
	}

	#[expect(unsafe_code, reason = "For FFI.")]
	/// # Get Vendor and Product Descriptors.
	pub(super) fn get_desc(dev: &Path) -> Result<Option<(u16, u16)>, RipRipError> {
		let Some(bsd_name) = dev.file_name()
			.and_then(|name| CString::new(name.as_bytes()).ok())
		else {
			log!(@trace [dev] "Failed to get device name.");
			return Ok(None);
		};

		// Find the specific IOMedia service for this BSD name.
		// Safety: this is an FFI call. Note `CFMutableDictionary` structurally
		// inherits from `CFDictionary`, so reinterpreting it as its base type
		// is entirely valid.
		let matching = unsafe {
			IOBSDNameMatching(kIOMainPortDefault, 0, bsd_name.as_ptr())
				.map(|v| CFRetained::cast_unchecked::<CFDictionary>(v))
		};
		if matching.is_none() {
			log!(@trace [dev] "Failed to create an IOKit matching dictionary.");
			return Ok(None);
		}

		let mut iterator = 0;
		// Safety: this is an FFI call.
		let res = unsafe {
			IOServiceGetMatchingServices(kIOMainPortDefault, matching, &raw mut iterator)
		};
		if res != kIOReturnSuccess {
			log!(@trace [dev] "IOServiceGetMatchingServices failed: 0x{res:08x}.");
			return Err(RipRipError::Bug("IOServiceGetMatchingServices"));
		}
		if iterator == 0 {
			log!(
				@trace
				[dev]
				"No matching services found for {}.",
				bsd_name.to_string_lossy(),
			);
			return Ok(None);
		}

		let media_service = IOIteratorNext(iterator);
		IOObjectRelease(iterator);

		if media_service == 0 { return Ok(None); }

		let vid_opt = get_numeric_property(media_service, "idVendor");
		let pid_opt = get_numeric_property(media_service, "idProduct");
		let option = vid_opt.zip(pid_opt);
		if option.is_none() {
			log!(@trace [dev, media_service] "Missing USB device properties.");
		}

		// Clean up.
		IOObjectRelease(media_service);

		// Done!
		Ok(option)
	}
}

#[cfg(target_os = "linux")]
/// # Linux.
mod linux {
	use crate::macros::log;
	use std::path::Path;

	#[must_use]
	/// # Get Vendor and Product Descriptors.
	pub(super) fn get_desc(dev: &Path) -> Option<(u16, u16)> {
		// Resolve the block path.
		let Some(name) = dev.file_name() else {
			log!(@trace [dev] "Failed to get device name.");
			return None;
		};
		let Ok(mut path) = std::fs::canonicalize(format!(
			"/sys/class/block/{name}/device",
			name=name.display(),
		)) else {
			log!(
				@trace
				[dev]
				"Missing \"/sys/class/block/{name}/device\".",
				name=name.display(),
			);
			return None;
		};

		// Block device symlinks point to obscure trees requiring a bit of
		// manual traversal to locate the `idVendor` and `idProduct` files.
		loop {
			// Don't traverse above /sys.
			if ! path.starts_with("/sys") { return None; }

			// If there are vendor and product IDs, decode and return.
			if
				let Ok(vid) = std::fs::read_to_string(path.join("idVendor")) &&
				let Ok(vid) = u16::from_str_radix(vid.trim(), 16) &&
				let Ok(pid) = std::fs::read_to_string(path.join("idProduct")) &&
				let Ok(pid) = u16::from_str_radix(pid.trim(), 16)
			{
				let sys = path;
				log!(@trace [dev, sys] "Found device descriptors.");
				return Some((vid, pid));
			}

			// We're out of parts!
			if ! path.pop() { return None; }
		}
	}
}

#[allow(
	clippy::allow_attributes,
	clippy::unnecessary_wraps,
	reason = "For consistency across targets.",
)]
/// # Get Vendor and Product Descriptors.
///
/// Retrieve the vendor and product ID for the given device path, or `None`
/// if the path is invalid, not a USB device, or the target OS is unsupported.
///
/// ## Errors
///
/// This will return an error if `IOServiceGetMatchingServices` fails for a
/// Mac target.
pub(super) fn get_desc<P>(dev: &P) -> Result<Option<(u16, u16)>, RipRipError>
where P: AsRef<Path> {
	cfg_select! {
		target_os = "macos" => macos::get_desc(dev.as_ref()),
		target_os = "linux" => Ok(linux::get_desc(dev.as_ref())),
		_ => Ok(None),
	}
}
