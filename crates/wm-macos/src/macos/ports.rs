//! Which port each external display is plugged into (spec 5.1, MON-2 by port).
//!
//! The IOKit registry has one `IOMobileFramebufferShim` service per display controller;
//! for a connected external display its `DisplayAttributes` dictionary holds `PortID` and
//! the display's vendor/model/serial numbers. These are public IOKit calls, but the class
//! and key names are undocumented: if they ever disappear we return nothing and monitor
//! identity falls back to the display UUID.

use std::ffi::{CStr, c_char, c_void};
use std::ptr::NonNull;

use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType};

use crate::monitors::PortEntry;
use crate::wm_log;

type IoObject = u32;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingServices(
        main_port: u32,
        matching: *mut c_void,
        existing: *mut IoObject,
    ) -> i32;
    fn IOIteratorNext(iterator: IoObject) -> IoObject;
    fn IORegistryEntryCreateCFProperty(
        entry: IoObject,
        key: &CFString,
        allocator: *const c_void,
        options: u32,
    ) -> *mut CFType;
    fn IOObjectRelease(object: IoObject) -> i32;
}

/// Every external display the registry reports with a `PortID`.
pub(crate) fn port_entries() -> Vec<PortEntry> {
    const CLASS: &CStr = c"IOMobileFramebufferShim";
    // SAFETY: `CLASS` is a valid C string; the result is a +1 dictionary or null.
    let matching = unsafe { IOServiceMatching(CLASS.as_ptr()) };
    if matching.is_null() {
        return Vec::new();
    }
    let mut iterator: IoObject = 0;
    // SAFETY: port 0 means "the default main port"; this call consumes `matching`.
    let status = unsafe { IOServiceGetMatchingServices(0, matching, &mut iterator) };
    if status != 0 {
        wm_log!("IOServiceGetMatchingServices failed: {status}");
        return Vec::new();
    }
    let key = CFString::from_str("DisplayAttributes");
    let mut entries = Vec::new();
    loop {
        // SAFETY: `iterator` is valid until released below; 0 means the end.
        let service = unsafe { IOIteratorNext(iterator) };
        if service == 0 {
            break;
        }
        // SAFETY: `service` is a valid registry entry; the result is +1 (Create rule) or null.
        let raw = unsafe { IORegistryEntryCreateCFProperty(service, &key, std::ptr::null(), 0) };
        // SAFETY: we own one reference to `service` from the iterator.
        unsafe { IOObjectRelease(service) };
        let Some(raw) = NonNull::new(raw) else {
            continue;
        };
        // SAFETY: we own that +1 reference; `CFRetained` releases it.
        let attributes = unsafe { CFRetained::from_raw(raw) };
        if let Some(entry) = parse(&attributes) {
            entries.push(entry);
        }
    }
    // SAFETY: we own the iterator.
    unsafe { IOObjectRelease(iterator) };
    entries
}

/// Reads `PortID` and `ProductAttributes.{LegacyManufacturerID, ProductID, SerialNumber}`.
fn parse(attributes: &CFType) -> Option<PortEntry> {
    let dict = attributes.downcast_ref::<CFDictionary>()?;
    // SAFETY: registry dictionaries have CFString keys and CF object values.
    let dict: &CFDictionary<CFString, CFType> = unsafe { dict.cast_unchecked() };
    let port = number(dict, "PortID")?;
    let product = dict.get(&CFString::from_str("ProductAttributes"))?;
    let product = product.downcast_ref::<CFDictionary>()?;
    // SAFETY: as above.
    let product: &CFDictionary<CFString, CFType> = unsafe { product.cast_unchecked() };
    Some(PortEntry {
        vendor: number(product, "LegacyManufacturerID").unwrap_or(0),
        model: number(product, "ProductID").unwrap_or(0),
        // Monitors without a serial number report none here and 0 to CoreGraphics.
        serial: number(product, "SerialNumber").unwrap_or(0),
        port,
    })
}

fn number(dict: &CFDictionary<CFString, CFType>, key: &str) -> Option<u32> {
    let value = dict.get(&CFString::from_str(key))?;
    let n = value.downcast_ref::<CFNumber>()?.as_i64()?;
    u32::try_from(n).ok()
}
