//! Implementation of [`get_idle_time`] for macOS.

use std::{io, mem::size_of, ptr::null_mut, time::Duration};

use anyhow::anyhow;
use core_foundation_sys::{
    base::{CFGetTypeID, CFRange, CFRelease, CFTypeRef, kCFAllocatorDefault},
    data::{CFDataGetBytes, CFDataGetTypeID},
    dictionary::CFDictionaryGetValueIfPresent,
    number::{CFNumberGetTypeID, CFNumberGetValue, kCFNumberSInt64Type},
    string::{CFStringCreateWithCString, kCFStringEncodingUTF8},
};
use io_kit_sys::{
    IOIteratorNext, IOMasterPort, IOObjectRelease, IORegistryEntryCreateCFProperties,
    IOServiceGetMatchingServices, IOServiceMatching,
};
use mach2::{
    kern_return::KERN_SUCCESS,
    port::{MACH_PORT_NULL, mach_port_t},
};

use crate::Result;

/// Get the idle time of a user.
///
/// # Errors
///
/// Errors if a system call fails.
#[inline]
#[expect(clippy::as_conversions, reason = "manually validated")]
#[expect(clippy::cast_sign_loss, reason = "manually validated")]
#[expect(clippy::cast_possible_wrap, reason = "manually validated")]
#[expect(clippy::host_endian_bytes, reason = "manually validated")]
pub fn get_idle_time() -> Result<Duration> {
    let mut ns = 0_u64;
    let mut port: mach_port_t = 0;
    let mut iter = 0;
    let mut properties = null_mut();

    // SAFETY: IOMasterPort is a well-defined IOKit function.
    let port_result = unsafe { IOMasterPort(MACH_PORT_NULL, &raw mut port) };
    if port_result != KERN_SUCCESS {
        return Err(anyhow!(
            "Unable to open mach port: {}",
            io::Error::last_os_error()
        ));
    }

    // SAFETY: IOServiceMatching returns a dictionary.
    let matching = unsafe { IOServiceMatching(c"IOHIDSystem".as_ptr()) };
    // SAFETY: IOServiceGetMatchingServices consumes the matching dictionary.
    let service_result = unsafe { IOServiceGetMatchingServices(port, matching, &raw mut iter) };
    if service_result != KERN_SUCCESS {
        return Err(anyhow!(
            "Unable to lookup IOHIDSystem: {}",
            io::Error::last_os_error()
        ));
    }

    if iter == 0 {
        return Err(anyhow!("No IOHIDSystem iterator"));
    }

    // SAFETY: iter is a valid iterator from IOServiceGetMatchingServices.
    let entry = unsafe { IOIteratorNext(iter) };
    if entry == 0 {
        // SAFETY: iter is a valid IOKit object.
        unsafe {
            IOObjectRelease(iter);
        }
        return Err(anyhow!("No IOHIDSystem entry"));
    }

    // SAFETY: entry is a valid registry entry, properties is written to by the function.
    let prop_res = unsafe {
        IORegistryEntryCreateCFProperties(entry, &raw mut properties, kCFAllocatorDefault, 0)
    };

    if prop_res == KERN_SUCCESS {
        // SAFETY: kCFAllocatorDefault and kCFStringEncodingUTF8 are valid constants.
        let prop_name_cf = unsafe {
            CFStringCreateWithCString(
                kCFAllocatorDefault,
                c"HIDIdleTime".as_ptr(),
                kCFStringEncodingUTF8,
            )
        };

        let mut value: CFTypeRef = null_mut();
        // SAFETY: properties is a valid dictionary, prop_name_cf is a valid string.
        let present = unsafe {
            CFDictionaryGetValueIfPresent(properties, prop_name_cf.cast(), &raw mut value)
        };
        // SAFETY: prop_name_cf was created by CFStringCreateWithCString above.
        unsafe {
            CFRelease(prop_name_cf.cast());
        }

        if present != 0 {
            // SAFETY: value was set by CFDictionaryGetValueIfPresent when present != 0.
            let value_type = unsafe { CFGetTypeID(value) };
            // SAFETY: CFDataGetTypeID returns the type ID for CFData.
            let data_type = unsafe { CFDataGetTypeID() };
            // SAFETY: CFNumberGetTypeID returns the type ID for CFNumber.
            let number_type = unsafe { CFNumberGetTypeID() };

            if value_type == data_type {
                let mut buf = [0_u8; size_of::<i64>()];
                let range = CFRange {
                    location: 0,
                    length: size_of::<i64>() as isize,
                };
                // SAFETY: value is a valid CFData, range and buffer are correctly sized.
                unsafe {
                    CFDataGetBytes(value.cast(), range, buf.as_mut_ptr());
                }
                ns = i64::from_ne_bytes(buf) as u64;
            } else if value_type == number_type {
                let mut val: i64 = 0;
                // SAFETY: value is a valid CFNumber of type SInt64.
                unsafe {
                    CFNumberGetValue(value.cast(), kCFNumberSInt64Type, (&raw mut val).cast());
                }
                ns = val as u64;
            }
        }

        // SAFETY: properties was created by IORegistryEntryCreateCFProperties.
        unsafe {
            CFRelease(properties.cast());
        }
    }

    // SAFETY: entry is a valid IOKit object that has not been released yet.
    unsafe {
        IOObjectRelease(entry);
    }
    // SAFETY: iter is a valid IOKit object that has not been released yet.
    unsafe {
        IOObjectRelease(iter);
    }

    Ok(Duration::from_nanos(ns))
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unit tests")]
mod test {
    use super::*;

    #[test]
    fn does_not_panic() {
        get_idle_time().unwrap();
    }
}
