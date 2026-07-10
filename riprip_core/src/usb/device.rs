/*!
# Rip Rip Hooray: USB Device.

Provides cross-platform lookup to get a USB drive descriptor (Vendor and Product IDs)
from an OS device path.
*/

use std::path::Path;

use crate::RipRipError;

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    use objc2_core_foundation::{
        kCFAllocatorDefault, CFDictionary, CFNumber, CFRetained, CFString, CFType,
    };
    use objc2_io_kit::{
        kIOMainPortDefault, kIORegistryIterateParents, kIORegistryIterateRecursively,
        kIOReturnSuccess, kIOServicePlane, IOBSDNameMatching, IOIteratorNext, IOObjectRelease,
        IORegistryEntrySearchCFProperty, IOServiceGetMatchingServices,
    };

    #[expect(unsafe_code, reason = "For FFI.")]
    fn get_numeric_property(media_service: u32, key: &str) -> Option<u16> {
        let cf_key = CFString::from_str(key);

        let prop = unsafe {
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
    pub(super) fn get_desc<P>(dev_path: &P) -> Result<Option<(u16, u16)>, RipRipError>
    where
        P: AsRef<Path>,
    {
        let Some(bsd_name) = dev_path
            .as_ref()
            .file_name()
            .and_then(|name| CString::new(name.as_bytes()).ok())
        else {
            return Ok(None);
        };

        // Find the specific IOMedia service for this BSD name.
        let matching_mut = unsafe { IOBSDNameMatching(kIOMainPortDefault, 0, bsd_name.as_ptr()) };
        if matching_mut.is_none() {
            return Ok(None);
        }

        let matching = matching_mut.map(|dict| {
            // `CFMutableDictionary` structurally inherits from `CFDictionary`, so reinterpreting it
            // as its base type is entirely valid.
            unsafe { CFRetained::cast_unchecked::<CFDictionary>(dict) }
        });

        let mut iterator = 0;
        let res =
            unsafe { IOServiceGetMatchingServices(kIOMainPortDefault, matching, &mut iterator) };
        if res != kIOReturnSuccess {
            return Err(RipRipError::Bug("IOServiceGetMatchingServices failed."));
        }
        if iterator == 0 {
            return Ok(None);
        }

        let media_service = IOIteratorNext(iterator);
        IOObjectRelease(iterator);

        if media_service == 0 {
            return Ok(None);
        }

        let vid_opt = get_numeric_property(media_service, "idVendor");
        let pid_opt = get_numeric_property(media_service, "idProduct");
        let result = Ok(vid_opt.zip(pid_opt));

        IOObjectRelease(media_service);

        result
    }
}

mod linux {
    use super::*;

    use std::fs;

    pub(super) fn get_desc<P>(dev: P) -> Option<(u16, u16)>
    where
        P: AsRef<Path>,
    {
        let name = dev.as_ref().file_name()?.to_str()?;

        let mut path = fs::canonicalize(format!("/sys/class/block/{name}/device")).ok()?;

        loop {
            if !path.starts_with("/sys") {
                return None;
            }

            let vid = fs::read_to_string(path.join("idVendor")).ok();
            let pid = fs::read_to_string(path.join("idProduct")).ok();

            if let (Some(vid), Some(pid)) = (vid, pid) {
                let ids = u16::from_str_radix(vid.trim(), 16)
                    .ok()
                    .zip(u16::from_str_radix(pid.trim(), 16).ok());

                if ids.is_some() {
                    return ids;
                }
            }

            if !path.pop() {
                return None;
            }
        }
    }
}

/// Retrieves the `(Vendor ID, Product ID)` for a device path (e.g. on macOS, `"/dev/disk4"`).
///
/// Returns `Ok(None)` if the path is invalid or not a USB device.
pub(super) fn get_desc<P>(dev: &P) -> Result<Option<(u16, u16)>, RipRipError>
where
    P: AsRef<Path>,
{
    #[cfg(target_os = "macos")]
    {
        return macos::get_desc(dev);
    }

    #[cfg(target_os = "linux")]
    {
        return Ok(linux::get_desc(dev));
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        return Ok(None);
    }
}
