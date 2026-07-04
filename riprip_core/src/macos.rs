use std::ffi::CString;
use std::path::Path;

use objc2_core_foundation::{
    kCFAllocatorDefault, CFDictionary, CFNumber, CFRetained, CFString, CFType,
};
use objc2_io_kit::{
    kIOMainPortDefault, kIORegistryIterateParents, kIORegistryIterateRecursively, kIOServicePlane,
    IOBSDNameMatching, IOIteratorNext, IOObjectRelease, IORegistryEntrySearchCFProperty,
    IOServiceGetMatchingServices,
};

#[expect(unsafe_code, reason = "For FFI.")]
/// Retrieves the `(Vendor ID, Product ID)` for a macOS device path (e.g., `"/dev/disk4"`).
///
/// Returns `None` if the path is invalid or not a USB device.
pub(super) fn get_device_desc<P>(dev_path: P) -> Option<(u16, u16)>
where
    P: AsRef<Path>,
{
    let file_name = dev_path.as_ref().file_name()?;

    let bsd_name = CString::new(file_name.to_string_lossy().into_owned()).ok()?;

    // Find the specific IOMedia service for this BSD name.
    let matching_mut = unsafe { IOBSDNameMatching(kIOMainPortDefault, 0, bsd_name.as_ptr()) };
    if matching_mut.is_none() {
        return None;
    }

    let matching: Option<CFRetained<CFDictionary>> = unsafe { std::mem::transmute(matching_mut) };

    let mut iterator = 0;
    let res = unsafe { IOServiceGetMatchingServices(kIOMainPortDefault, matching, &mut iterator) };
    if res != 0 || iterator == 0 {
        return None;
    }

    let media_service = IOIteratorNext(iterator);
    IOObjectRelease(iterator);

    if media_service == 0 {
        return None;
    }

    let vid_key = CFString::from_str("idVendor");
    let vid_prop = unsafe {
        IORegistryEntrySearchCFProperty(
            media_service,
            kIOServicePlane.as_ptr() as *mut _,
            Some(&vid_key),
            kCFAllocatorDefault,
            kIORegistryIterateRecursively | kIORegistryIterateParents,
        )
    };

    let pid_key = CFString::from_str("idProduct");
    let pid_prop = unsafe {
        IORegistryEntrySearchCFProperty(
            media_service,
            kIOServicePlane.as_ptr() as *mut _,
            Some(&pid_key),
            kCFAllocatorDefault,
            kIORegistryIterateRecursively | kIORegistryIterateParents,
        )
    };

    let props = vid_prop.zip(pid_prop);

    IOObjectRelease(media_service);

    props.and_then(|(v, p)| {
        let vid = CFType::downcast_ref::<CFNumber>(&v)?.as_i32()?;
        let pid = CFType::downcast_ref::<CFNumber>(&p)?.as_i32()?;

        Some((vid as u16, pid as u16))
    })
}
