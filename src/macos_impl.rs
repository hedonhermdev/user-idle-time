//! Implementation of [`get_idle_time`] for macOS.

use std::{mem::size_of, ptr::null_mut, time::Duration};

use anyhow::anyhow;
use core_foundation_sys::{
    base::{CFGetTypeID, CFRange, CFRelease, CFTypeRef, kCFAllocatorDefault},
    data::{CFDataGetBytes, CFDataGetLength, CFDataGetTypeID},
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

/// RAII guard for `IOKit` objects that calls `IOObjectRelease` on drop.
struct IoObject(mach_port_t);

impl Drop for IoObject {
    fn drop(&mut self) {
        // SAFETY: The wrapped handle is a valid IOKit object obtained from an IOKit function.
        unsafe {
            IOObjectRelease(self.0);
        }
    }
}

/// RAII guard for Core Foundation objects that calls `CFRelease` on drop.
struct CfGuard(CFTypeRef);

impl Drop for CfGuard {
    fn drop(&mut self) {
        // SAFETY: The wrapped pointer is a valid, non-null CF object with +1 retain.
        unsafe {
            CFRelease(self.0);
        }
    }
}

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
    let mut port: mach_port_t = 0;
    let mut iter = 0;
    let mut properties = null_mut();

    // SAFETY: IOMasterPort is a well-defined IOKit function.
    let port_result = unsafe { IOMasterPort(MACH_PORT_NULL, &raw mut port) };
    if port_result != KERN_SUCCESS {
        return Err(anyhow!(
            "Unable to open mach port (kern_return: {port_result})"
        ));
    }

    // SAFETY: IOServiceMatching returns a dictionary or null.
    let matching = unsafe { IOServiceMatching(c"IOHIDSystem".as_ptr()) };
    if matching.is_null() {
        return Err(anyhow!("IOServiceMatching returned null"));
    }
    // SAFETY: IOServiceGetMatchingServices consumes the matching dictionary
    // (releases it regardless of success or failure).
    let service_result = unsafe { IOServiceGetMatchingServices(port, matching, &raw mut iter) };
    if service_result != KERN_SUCCESS {
        return Err(anyhow!(
            "Unable to lookup IOHIDSystem (kern_return: {service_result})"
        ));
    }

    if iter == 0 {
        return Err(anyhow!("No IOHIDSystem iterator"));
    }
    let iter = IoObject(iter);

    // SAFETY: iter.0 is a valid iterator from IOServiceGetMatchingServices.
    let entry = unsafe { IOIteratorNext(iter.0) };
    if entry == 0 {
        return Err(anyhow!("No IOHIDSystem entry"));
    }
    let entry = IoObject(entry);

    // SAFETY: entry.0 is a valid registry entry, properties is written to on success.
    let prop_res = unsafe {
        IORegistryEntryCreateCFProperties(entry.0, &raw mut properties, kCFAllocatorDefault, 0)
    };
    if prop_res != KERN_SUCCESS {
        return Err(anyhow!(
            "IORegistryEntryCreateCFProperties failed (kern_return: {prop_res})"
        ));
    }
    // properties is now a valid CFMutableDictionary at +1 retain. Wrap in RAII guard.
    let properties = CfGuard(properties.cast());

    // SAFETY: arguments are valid constants.
    let prop_name_cf = unsafe {
        CFStringCreateWithCString(
            kCFAllocatorDefault,
            c"HIDIdleTime".as_ptr(),
            kCFStringEncodingUTF8,
        )
    };
    if prop_name_cf.is_null() {
        return Err(anyhow!("CFStringCreateWithCString returned null"));
    }
    let prop_name_cf = CfGuard(prop_name_cf.cast());

    let mut value: CFTypeRef = null_mut();
    // SAFETY: properties.0 is a valid dictionary, prop_name_cf.0 is a valid (non-null) string.
    let present = unsafe {
        CFDictionaryGetValueIfPresent(properties.0.cast(), prop_name_cf.0, &raw mut value)
    };
    // prop_name_cf is released by CfGuard drop at end of scope (or on early return).

    if present == 0 {
        return Err(anyhow!("HIDIdleTime property not found"));
    }

    // SAFETY: value was set by CFDictionaryGetValueIfPresent when present != 0.
    let value_type = unsafe { CFGetTypeID(value) };
    // SAFETY: CFDataGetTypeID returns the type ID for CFData.
    let data_type = unsafe { CFDataGetTypeID() };
    // SAFETY: CFNumberGetTypeID returns the type ID for CFNumber.
    let number_type = unsafe { CFNumberGetTypeID() };

    let ns;
    if value_type == data_type {
        // SAFETY: value is a valid CFData; CFDataGetLength returns its byte length.
        let data_len = unsafe { CFDataGetLength(value.cast()) };
        if data_len < size_of::<i64>() as isize {
            return Err(anyhow!(
                "HIDIdleTime CFData too short: expected {} bytes, got {data_len}",
                size_of::<i64>()
            ));
        }
        let mut buf = [0_u8; size_of::<i64>()];
        let range = CFRange {
            location: 0,
            length: size_of::<i64>() as isize,
        };
        // SAFETY: value is a valid CFData, range is within bounds (verified above),
        // and buffer is correctly sized.
        unsafe {
            CFDataGetBytes(value.cast(), range, buf.as_mut_ptr());
        }
        ns = i64::from_ne_bytes(buf) as u64;
    } else if value_type == number_type {
        let mut val: i64 = 0;
        // SAFETY: value is a valid CFNumber; buffer is correctly sized for kCFNumberSInt64Type.
        unsafe {
            CFNumberGetValue(value.cast(), kCFNumberSInt64Type, (&raw mut val).cast());
        }
        ns = val as u64;
    } else {
        return Err(anyhow!(
            "HIDIdleTime has unexpected CFType (type id: {value_type})"
        ));
    }

    // All RAII guards (properties, prop_name_cf, entry, iter) are released on drop.

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
