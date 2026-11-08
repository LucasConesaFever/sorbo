//! Small safe wrappers around Accessibility calls. Each one wraps exactly one `unsafe` call.
//!
//! AX follows Core Foundation's "Copy rule": values come back with a +1 reference count
//! that we own. Wrapping them in `CFRetained` releases them when dropped.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_application_services::{AXError, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{CFArray, CFBoolean, CFRetained, CFString, CFType, CGPoint, CGSize};
use wm_core::Rect;

/// Sets the timeout for every AX call this process makes (spec 3.4). A call to a hung app
/// fails after this long instead of blocking the WM.
pub(crate) fn set_global_timeout(seconds: f32) -> AXError {
    // SAFETY: creating the system-wide element has no preconditions.
    let system = unsafe { AXUIElement::new_system_wide() };
    // SAFETY: `system` is a valid AX element.
    unsafe { system.set_messaging_timeout(seconds) }
}

/// The AX element for an application.
pub(crate) fn app_element(pid: i32) -> CFRetained<AXUIElement> {
    // SAFETY: any pid is accepted; a dead pid yields an element whose calls fail.
    unsafe { AXUIElement::new_application(pid) }
}

/// Reads an attribute. `Err` carries the AX error (e.g. the element is gone).
pub(crate) fn attr(element: &AXUIElement, name: &str) -> Result<CFRetained<CFType>, AXError> {
    let name = CFString::from_str(name);
    let mut value: *const CFType = std::ptr::null();
    // SAFETY: `value` is a valid out-pointer for the duration of the call.
    let err = unsafe { element.copy_attribute_value(&name, NonNull::from(&mut value)) };
    if err != AXError::Success {
        return Err(err);
    }
    let value = NonNull::new(value as *mut CFType).ok_or(AXError::NoValue)?;
    // SAFETY: AX returned a +1 reference that nobody else will release.
    Ok(unsafe { CFRetained::from_raw(value) })
}

pub(crate) fn string_attr(element: &AXUIElement, name: &str) -> Option<String> {
    attr(element, name)
        .ok()?
        .downcast::<CFString>()
        .ok()
        .map(|s| s.to_string())
}

pub(crate) fn bool_attr(element: &AXUIElement, name: &str) -> Option<bool> {
    attr(element, name)
        .ok()?
        .downcast::<CFBoolean>()
        .ok()
        .map(|b| b.as_bool())
}

/// The AX elements in an array attribute such as `AXWindows`.
pub(crate) fn elements_attr(element: &AXUIElement, name: &str) -> Vec<CFRetained<AXUIElement>> {
    let Some(array) = attr(element, name)
        .ok()
        .and_then(|v| v.downcast::<CFArray>().ok())
    else {
        return Vec::new();
    };
    // SAFETY: viewing an untyped CFArray as an array of CFType is sound (every element is a
    // CF object); each one is then checked with `downcast`.
    let array: &CFArray<CFType> = unsafe { array.cast_unchecked() };
    array
        .iter()
        .filter_map(|item| item.downcast::<AXUIElement>().ok())
        .collect()
}

/// Whether an attribute can be set (e.g. `AXSize` on a resizable window).
pub(crate) fn settable(element: &AXUIElement, name: &str) -> Option<bool> {
    let name = CFString::from_str(name);
    let mut out: u8 = 0;
    // SAFETY: `out` is a valid, writable Boolean (u8) for the duration of the call.
    let err = unsafe { element.is_attribute_settable(&name, NonNull::from(&mut out)) };
    (err == AXError::Success).then_some(out != 0)
}

/// The window's frame (`AXPosition` + `AXSize`), top-left origin.
pub(crate) fn frame(element: &AXUIElement) -> Option<Rect> {
    let mut p = CGPoint::default();
    let mut s = CGSize::default();
    let pos = attr(element, "AXPosition")
        .ok()?
        .downcast::<AXValue>()
        .ok()?;
    let size = attr(element, "AXSize").ok()?.downcast::<AXValue>().ok()?;
    // SAFETY: `p` is a valid CGPoint to write into, matching the requested type.
    let ok_p = unsafe { pos.value(AXValueType::CGPoint, NonNull::from(&mut p).cast::<c_void>()) };
    // SAFETY: `s` is a valid CGSize to write into, matching the requested type.
    let ok_s = unsafe { size.value(AXValueType::CGSize, NonNull::from(&mut s).cast::<c_void>()) };
    (ok_p && ok_s).then(|| Rect::new(p.x, p.y, s.width, s.height))
}

pub(crate) fn set_position(element: &AXUIElement, x: f64, y: f64) -> AXError {
    let mut p = CGPoint::new(x, y);
    // SAFETY: `p` is a valid CGPoint for the duration of the call.
    let value = unsafe { AXValue::new(AXValueType::CGPoint, NonNull::from(&mut p).cast()) };
    set_value(element, "AXPosition", value)
}

pub(crate) fn set_size(element: &AXUIElement, width: f64, height: f64) -> AXError {
    let mut s = CGSize::new(width, height);
    // SAFETY: `s` is a valid CGSize for the duration of the call.
    let value = unsafe { AXValue::new(AXValueType::CGSize, NonNull::from(&mut s).cast()) };
    set_value(element, "AXSize", value)
}

fn set_value(element: &AXUIElement, name: &str, value: Option<CFRetained<AXValue>>) -> AXError {
    let Some(value) = value else {
        return AXError::Failure;
    };
    let name = CFString::from_str(name);
    // SAFETY: both are valid CF objects for the duration of the call.
    unsafe { element.set_attribute_value(&name, &value) }
}

/// The pid of the app that owns an element.
pub(crate) fn element_pid(element: &AXUIElement) -> Option<i32> {
    let mut pid: i32 = 0;
    // SAFETY: `pid` is a valid, writable i32 for the duration of the call.
    let err = unsafe { element.pid(NonNull::from(&mut pid)) };
    (err == AXError::Success).then_some(pid)
}

pub(crate) fn set_bool(element: &AXUIElement, name: &str, value: bool) -> AXError {
    let name = CFString::from_str(name);
    // SAFETY: both are valid CF objects for the duration of the call.
    unsafe { element.set_attribute_value(&name, CFBoolean::new(value)) }
}

/// Performs an action such as `AXRaise` or `AXPress`.
pub(crate) fn perform(element: &AXUIElement, action: &str) -> AXError {
    let action = CFString::from_str(action);
    // SAFETY: `action` is a valid CFString for the duration of the call.
    unsafe { element.perform_action(&action) }
}
