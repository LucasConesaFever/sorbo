//! Global hotkeys with Carbon's `RegisterEventHotKey` (spec 5.5).
//!
//! Carbon only fires for the exact combos we register, so it can't swallow normal typing,
//! and it needs no permission. None of this is in the objc2 crates, so the few functions we
//! need are declared by hand below.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_core_foundation::{CFData, CFRetained, CFString, CFType, Type};
use wm_core::{Action, ConfigError, Event, KeyCombo, Mode, combo_name};

use super::{Shared, info_ptr, run_or_defer};
use crate::keys::{
    KeyCode, TakenKeys, carbon_modifiers, find_code_with_shift, key_code, mode_keys,
};
use crate::wm_log;

type OSStatus = i32;
type EventTargetRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventHandlerProc =
    unsafe extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
#[derive(Default)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

/// Four-character codes from Carbon's headers.
const K_EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
const K_EVENT_PARAM_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
const TYPE_EVENT_HOT_KEY_ID: u32 = u32::from_be_bytes(*b"hkid");
/// Our hotkeys' signature, so we only react to our own.
const SIGNATURE: u32 = u32::from_be_bytes(*b"sorb");
/// `UCKeyTranslate`: ask what a key displays, ignoring dead keys.
const K_UC_KEY_ACTION_DISPLAY: u16 = 3;
const K_UC_KEY_TRANSLATE_NO_DEAD_KEYS: u32 = 1;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandlerProc,
        num_types: usize,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut EventHandlerRef,
    ) -> OSStatus;
    fn UnregisterEventHotKey(hotkey: EventHotKeyRef) -> OSStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: EventTargetRef,
        options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> OSStatus;
    fn GetEventParameter(
        event: EventRef,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        buffer_size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> OSStatus;
    fn TISCopyCurrentKeyboardLayoutInputSource() -> *mut CFType;
    fn TISGetInputSourceProperty(source: *const CFType, key: *const CFString) -> *const CFType;
    static kTISPropertyUnicodeKeyLayoutData: *const CFString;
    fn LMGetKbdType() -> u8;
    fn UCKeyTranslate(
        layout: *const c_void,
        virtual_key_code: u16,
        key_action: u16,
        modifier_key_state: u32,
        keyboard_type: u32,
        key_translate_options: u32,
        dead_key_state: *mut u32,
        max_len: usize,
        actual_len: *mut usize,
        unicode: *mut u16,
    ) -> OSStatus;
}

/// Installs the handler Carbon calls for our hotkeys. Once per process.
pub(crate) fn install_handler(shared: &'static RefCell<Shared>) {
    let info = info_ptr(shared);
    let spec = EventTypeSpec {
        event_class: K_EVENT_CLASS_KEYBOARD,
        event_kind: K_EVENT_HOT_KEY_PRESSED,
    };
    let mut handler: EventHandlerRef = std::ptr::null_mut();
    // SAFETY: `spec` and `handler` are valid for the call; `info` is the shared-state
    // pointer, valid as long as `MacPlatform` lives.
    let status = unsafe {
        InstallEventHandler(
            GetApplicationEventTarget(),
            on_hotkey,
            1,
            &spec,
            info,
            &mut handler,
        )
    };
    if status != 0 {
        wm_log!("InstallEventHandler failed: {status}; no keybinding will work");
    }
}

/// Replaces every registered hotkey with `bindings` (CFG-4). Returns one error per binding
/// macOS can't register: its key isn't on the current keyboard layout, or the combo is taken.
///
/// Only the config's own hotkeys are replaced: the resize mode's keys, if the mode is on,
/// stay registered (MODE-5).
pub(crate) fn apply_bindings(s: &mut Shared, bindings: &[(KeyCombo, Action)]) -> Vec<ConfigError> {
    let old = std::mem::take(&mut s.hotkey_refs);
    unregister(s, old);

    let layout = KeyboardLayout::current();
    let mut taken = TakenKeys::default();
    let mut refused = Vec::new();
    let mut refuse = |combo: &KeyCombo, why: String| {
        let message = format!("{}: {why}", combo_name(combo));
        wm_log!("config: {message}");
        refused.push(ConfigError {
            line: None,
            message,
        });
    };
    for (combo, action) in bindings {
        let code = match key_code(combo.key) {
            KeyCode::Fixed(code) => code,
            KeyCode::OnLayout(c) => {
                let found = layout
                    .as_ref()
                    .and_then(|l| find_code_with_shift(c, |code, shift| l.translate(code, shift)));
                match found {
                    Some(code) => code,
                    None => {
                        refuse(combo, format!("no \"{c}\" key on this keyboard layout"));
                        continue;
                    }
                }
            }
        };
        if let Err(first) = taken.claim(code, carbon_modifiers(combo.mods), &combo_name(combo)) {
            refuse(
                combo,
                format!("same key as {first} on this keyboard layout"),
            );
            continue;
        }
        match register(s, code, carbon_modifiers(combo.mods), action) {
            Ok(hotkey) => s.hotkey_refs.push(hotkey),
            Err(status) => refuse(
                combo,
                format!("macOS refused it (already used by another app or the system?): {status}"),
            ),
        }
    }
    wm_log!("registered {} hotkeys", s.hotkey_refs.len());
    refused
}

/// MODE-1, MODE-5: registers the resize mode's fixed keys while the mode is on, and only
/// then, so outside the mode apps get their arrows, `h j k l`, Escape and Enter.
pub(crate) fn set_mode(s: &mut Shared, mode: Mode) {
    match mode {
        Mode::Resize if s.mode_hotkeys.is_empty() => {
            for (combo, action) in mode_keys() {
                let KeyCode::Fixed(code) = key_code(combo.key) else {
                    continue;
                };
                match register(s, code, carbon_modifiers(combo.mods), &action) {
                    Ok(hotkey) => s.mode_hotkeys.push(hotkey),
                    Err(status) => wm_log!(
                        "resize mode: macOS refused {}: {status}",
                        combo_name(&combo)
                    ),
                }
            }
        }
        Mode::Resize => {}
        Mode::Normal => {
            let keys = std::mem::take(&mut s.mode_hotkeys);
            unregister(s, keys);
        }
    }
}

/// A registered hotkey: its id in `Shared::hotkeys` and Carbon's reference.
pub(crate) type Hotkey = (u32, EventHotKeyRef);

/// Registers one hotkey that runs `action`, or returns Carbon's error status.
fn register(s: &mut Shared, code: u16, modifiers: u32, action: &Action) -> Result<Hotkey, i32> {
    // Reserve the id now; it's removed again if macOS refuses the hotkey.
    let action_id = s.hotkeys.add(action.clone());
    let id = EventHotKeyID {
        signature: SIGNATURE,
        id: action_id,
    };
    let mut hotkey: EventHotKeyRef = std::ptr::null_mut();
    // SAFETY: `hotkey` is a valid out-pointer; the target is the application target.
    let status = unsafe {
        RegisterEventHotKey(
            u32::from(code),
            modifiers,
            id,
            GetApplicationEventTarget(),
            0,
            &mut hotkey,
        )
    };
    if status != 0 {
        s.hotkeys.remove(action_id);
        return Err(status);
    }
    Ok((action_id, hotkey))
}

/// Unregisters `hotkeys` and forgets their actions.
fn unregister(s: &mut Shared, hotkeys: Vec<Hotkey>) {
    for (id, hotkey) in hotkeys {
        // SAFETY: each ref came from a successful `RegisterEventHotKey` and is unregistered
        // only once (the caller took it out of `Shared`).
        unsafe { UnregisterEventHotKey(hotkey) };
        s.hotkeys.remove(id);
    }
}

/// Called by Carbon when one of our hotkeys is pressed.
unsafe extern "C" fn on_hotkey(
    _call: EventHandlerCallRef,
    event: EventRef,
    info: *mut c_void,
) -> OSStatus {
    let mut id = EventHotKeyID::default();
    // SAFETY: `id` is a valid buffer of the requested type and size.
    let status = unsafe {
        GetEventParameter(
            event,
            K_EVENT_PARAM_DIRECT_OBJECT,
            TYPE_EVENT_HOT_KEY_ID,
            std::ptr::null_mut(),
            std::mem::size_of::<EventHotKeyID>(),
            std::ptr::null_mut(),
            (&mut id as *mut EventHotKeyID).cast(),
        )
    };
    if status != 0 || id.signature != SIGNATURE {
        return status;
    }
    let hotkey = id.id;
    // SAFETY: `info` is the shared-state pointer registered in `install_handler`.
    unsafe {
        run_or_defer(info, "shortcut", move |s| {
            if let Some(action) = s.hotkeys.get(hotkey).cloned() {
                s.push(Event::Action(action));
            }
        })
    };
    0
}

/// The current keyboard layout's key-to-character table.
struct KeyboardLayout {
    /// Keeps the input source (and so the layout data) alive.
    _source: CFRetained<CFType>,
    data: CFRetained<CFData>,
}

impl KeyboardLayout {
    fn current() -> Option<KeyboardLayout> {
        // SAFETY: no arguments; returns a +1 reference (Copy rule) or null.
        let source = NonNull::new(unsafe { TISCopyCurrentKeyboardLayoutInputSource() })?;
        // SAFETY: we own that reference; `CFRetained` releases it on drop.
        let source = unsafe { CFRetained::from_raw(source) };
        // SAFETY: valid source and key; the result isn't owned (Get rule) or is null.
        let data = unsafe { TISGetInputSourceProperty(&*source, kTISPropertyUnicodeKeyLayoutData) };
        // SAFETY: a non-null result is a CFData that lives as long as `source`.
        let data = unsafe { data.as_ref() }?.downcast_ref::<CFData>()?.retain();
        Some(KeyboardLayout {
            _source: source,
            data,
        })
    }

    /// The character key `code` types with no modifiers (or Shift only), if exactly one.
    fn translate(&self, code: u16, shift: bool) -> Option<char> {
        // `UCKeyTranslate` takes Carbon modifier flags shifted right by 8 (shiftKey >> 8).
        let modifier_state = if shift { 0x02 } else { 0 };
        let mut dead_keys = 0u32;
        let mut buffer = [0u16; 4];
        let mut len = 0usize;
        // SAFETY: the layout pointer comes from live CFData; every out-pointer is valid and
        // `buffer` holds `buffer.len()` UTF-16 units.
        let status = unsafe {
            UCKeyTranslate(
                self.data.byte_ptr().cast(),
                code,
                K_UC_KEY_ACTION_DISPLAY,
                modifier_state,
                u32::from(LMGetKbdType()),
                K_UC_KEY_TRANSLATE_NO_DEAD_KEYS,
                &mut dead_keys,
                buffer.len(),
                &mut len,
                buffer.as_mut_ptr(),
            )
        };
        if status != 0 || len != 1 {
            return None;
        }
        char::from_u32(u32::from(buffer[0]))
    }
}
