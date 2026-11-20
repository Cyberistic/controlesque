//! macOS IOKit physical keyboard seizer.
//!
//! Uses the exact same OS-level mechanism as Karabiner-Elements (`pqrs::osx::iokit_hid_manager`):
//! discovers `IOHIDDevice` services via `IOServiceAddMatchingNotification`, creates standalone
//! `IOHIDDeviceRef` handles via `IOHIDDeviceCreate`, and opens each physical keyboard exclusively
//! with `IOHIDDeviceOpen(device, kIOHIDOptionsTypeSeizeDevice)`.
//!
//! (Using `IOHIDManagerOpen` with `kIOHIDOptionsTypeNone` would pre-open devices non-exclusively,
//! preventing a subsequent `kIOHIDOptionsTypeSeizeDevice` open from taking exclusive control.)

#![allow(non_camel_case_types, non_upper_case_globals)]

use crate::fnmap::FnUsageMap;
use crate::hid::HidReport;
use crate::remap::{Action, Remapper, escape_tap_reports};
use crate::vhidd::{VIRTUAL_PRODUCT_ID, VIRTUAL_VENDOR_ID, VirtualKeyboardClient};
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

type CFAllocatorRef = *const c_void;
type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFNumberRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFMutableDictionaryRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CFIndex = isize;
type CFTypeID = usize;
type CFStringEncoding = u32;
type CFNumberType = isize;
type CFTimeInterval = f64;
type Boolean = u8;

type mach_port_t = u32;
type io_object_t = u32;
type io_service_t = io_object_t;
type io_registry_entry_t = io_object_t;
type io_iterator_t = io_object_t;
type IOReturn = i32;
type kern_return_t = i32;
type IOOptionBits = u32;
type IONotificationPortRef = *mut c_void;
type IOHIDDeviceRef = *mut c_void;
type IOHIDValueRef = *const c_void;
type IOHIDElementRef = *const c_void;

type IOServiceMatchingCallback = extern "C" fn(refcon: *mut c_void, iterator: io_iterator_t);

type IOHIDValueCallback = extern "C" fn(
    context: *mut c_void,
    result: IOReturn,
    sender: *mut c_void,
    value: IOHIDValueRef,
);

const kCFStringEncodingUTF8: CFStringEncoding = 0x0800_0100;
const kCFNumberSInt32Type: CFNumberType = 3;
const kCFNumberSInt64Type: CFNumberType = 4;

const kIOMainPortDefault: mach_port_t = 0;
const kIOReturnSuccess: IOReturn = 0;
const kIOReturnNotPermitted: IOReturn = 0xe000_02e2u32 as i32;
const kIOHIDOptionsTypeNone: IOOptionBits = 0;
const kIOHIDOptionsTypeSeizeDevice: IOOptionBits = 1;
const K_IO_REGISTRY_ITERATE_RECURSIVELY: IOOptionBits = 1;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: CFStringRef;
    static kCFPreferencesAnyApplication: CFStringRef;
    static kCFPreferencesCurrentUser: CFStringRef;
    static kCFPreferencesAnyHost: CFStringRef;

    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    fn CFStringGetTypeID() -> CFTypeID;
    fn CFNumberGetTypeID() -> CFTypeID;
    fn CFBooleanGetTypeID() -> CFTypeID;
    fn CFBooleanGetValue(boolean: CFTypeRef) -> Boolean;

    fn CFPreferencesCopyValue(
        key: CFStringRef,
        application_id: CFStringRef,
        user_name: CFStringRef,
        host_name: CFStringRef,
    ) -> CFTypeRef;

    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        c_str: *const c_char,
        encoding: CFStringEncoding,
    ) -> CFStringRef;

    fn CFStringGetCString(
        the_string: CFStringRef,
        buffer: *mut c_char,
        buffer_size: CFIndex,
        encoding: CFStringEncoding,
    ) -> Boolean;

    fn CFNumberCreate(
        allocator: CFAllocatorRef,
        the_type: CFNumberType,
        value_ptr: *const c_void,
    ) -> CFNumberRef;

    fn CFNumberGetValue(
        number: CFNumberRef,
        the_type: CFNumberType,
        value_ptr: *mut c_void,
    ) -> Boolean;

    fn CFDictionarySetValue(
        the_dict: CFMutableDictionaryRef,
        key: *const c_void,
        value: *const c_void,
    );

    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRemoveSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRunInMode(
        mode: CFStringRef,
        seconds: CFTimeInterval,
        return_after_source_handled: Boolean,
    ) -> i32;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IONotificationPortCreate(main_port: mach_port_t) -> IONotificationPortRef;
    fn IONotificationPortDestroy(notify: IONotificationPortRef);
    fn IONotificationPortGetRunLoopSource(notify: IONotificationPortRef) -> CFRunLoopSourceRef;

    fn IOServiceMatching(name: *const c_char) -> CFMutableDictionaryRef;
    fn IOServiceAddMatchingNotification(
        notify_port: IONotificationPortRef,
        notification_type: *const c_char,
        matching: CFDictionaryRef,
        callback: IOServiceMatchingCallback,
        ref_con: *mut c_void,
        notification: *mut io_iterator_t,
    ) -> kern_return_t;

    fn IOIteratorNext(iterator: io_iterator_t) -> io_object_t;
    fn IOObjectRelease(object: io_object_t) -> kern_return_t;
    fn IORegistryEntryGetRegistryEntryID(
        entry: io_registry_entry_t,
        entry_id: *mut u64,
    ) -> kern_return_t;
    fn IORegistryEntrySearchCFProperty(
        entry: io_registry_entry_t,
        plane: *const c_char,
        key: CFStringRef,
        allocator: CFAllocatorRef,
        options: IOOptionBits,
    ) -> CFTypeRef;

    fn IOHIDDeviceCreate(allocator: CFAllocatorRef, service: io_service_t) -> IOHIDDeviceRef;
    fn IOHIDDeviceOpen(device: IOHIDDeviceRef, options: IOOptionBits) -> IOReturn;
    fn IOHIDDeviceClose(device: IOHIDDeviceRef, options: IOOptionBits) -> IOReturn;
    fn IOHIDDeviceGetProperty(device: IOHIDDeviceRef, key: CFStringRef) -> CFTypeRef;
    fn IOHIDDeviceRegisterInputValueCallback(
        device: IOHIDDeviceRef,
        callback: IOHIDValueCallback,
        context: *mut c_void,
    );
    fn IOHIDDeviceScheduleWithRunLoop(
        device: IOHIDDeviceRef,
        run_loop: CFRunLoopRef,
        run_loop_mode: CFStringRef,
    );
    fn IOHIDDeviceUnscheduleFromRunLoop(
        device: IOHIDDeviceRef,
        run_loop: CFRunLoopRef,
        run_loop_mode: CFStringRef,
    );

    fn IOHIDValueGetElement(value: IOHIDValueRef) -> IOHIDElementRef;
    fn IOHIDValueGetLength(value: IOHIDValueRef) -> CFIndex;
    fn IOHIDValueGetIntegerValue(value: IOHIDValueRef) -> CFIndex;
    fn IOHIDElementGetType(element: IOHIDElementRef) -> u32;
    fn IOHIDElementGetUsagePage(element: IOHIDElementRef) -> u32;
    fn IOHIDElementGetUsage(element: IOHIDElementRef) -> u32;
}

struct CfOwned(CFTypeRef);

impl Drop for CfOwned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

fn cf_string(s: &str) -> CfOwned {
    let c = CString::new(s).expect("valid C string");
    let r = unsafe { CFStringCreateWithCString(ptr::null(), c.as_ptr(), kCFStringEncodingUTF8) };
    CfOwned(r)
}

fn cf_number_i32(v: i32) -> CfOwned {
    let r = unsafe {
        CFNumberCreate(
            ptr::null(),
            kCFNumberSInt32Type,
            (&v as *const i32).cast::<c_void>(),
        )
    };
    CfOwned(r)
}

fn make_keyboard_matching_dict() -> CFMutableDictionaryRef {
    let class_name = CString::new("IOHIDDevice").unwrap();
    let dict = unsafe { IOServiceMatching(class_name.as_ptr()) };
    let page_key = cf_string("DeviceUsagePage");
    let page_val = cf_number_i32(0x01); // kHIDPage_GenericDesktop
    let usage_key = cf_string("DeviceUsage");
    let usage_val = cf_number_i32(0x06); // kHIDUsage_GD_Keyboard
    unsafe {
        CFDictionarySetValue(dict, page_key.0, page_val.0);
        CFDictionarySetValue(dict, usage_key.0, usage_val.0);
    }
    dict
}

fn device_string_prop(device: IOHIDDeviceRef, key: &str) -> Option<String> {
    let k = cf_string(key);
    let prop = unsafe { IOHIDDeviceGetProperty(device, k.0) };
    if prop.is_null() || unsafe { CFGetTypeID(prop) != CFStringGetTypeID() } {
        return None;
    }
    let mut buf = [0u8; 256];
    let ok = unsafe {
        CFStringGetCString(
            prop,
            buf.as_mut_ptr().cast::<c_char>(),
            buf.len() as CFIndex,
            kCFStringEncodingUTF8,
        )
    };
    if ok == 0 {
        return None;
    }
    let cstr = unsafe { CStr::from_ptr(buf.as_ptr().cast::<c_char>()) };
    Some(cstr.to_string_lossy().into_owned())
}

fn device_i64_prop(device: IOHIDDeviceRef, key: &str) -> Option<i64> {
    let k = cf_string(key);
    let prop = unsafe { IOHIDDeviceGetProperty(device, k.0) };
    if prop.is_null() || unsafe { CFGetTypeID(prop) != CFNumberGetTypeID() } {
        return None;
    }
    let mut out: i64 = 0;
    let ok = unsafe {
        CFNumberGetValue(
            prop,
            kCFNumberSInt64Type,
            (&mut out as *mut i64).cast::<c_void>(),
        )
    };
    if ok != 0 { Some(out) } else { None }
}

/// macOS' "Use F1, F2, etc. keys as standard function keys" preference.
///
/// The built-in keyboard's driver reads this to decide whether the top row
/// sends F1..F12 or the media keys. Seizing the device bypasses that driver, so
/// the remapper has to honor the same setting.
fn use_fkeys_as_standard_function_keys() -> bool {
    let key = cf_string("com.apple.keyboard.fnState");
    let value = unsafe {
        CFPreferencesCopyValue(
            key.0,
            kCFPreferencesAnyApplication,
            kCFPreferencesCurrentUser,
            kCFPreferencesAnyHost,
        )
    };
    if value.is_null() {
        return false;
    }
    let value = CfOwned(value);
    let type_id = unsafe { CFGetTypeID(value.0) };
    if type_id == unsafe { CFBooleanGetTypeID() } {
        return unsafe { CFBooleanGetValue(value.0) } != 0;
    }
    if type_id == unsafe { CFNumberGetTypeID() } {
        let mut out: i64 = 0;
        let ok = unsafe {
            CFNumberGetValue(
                value.0,
                kCFNumberSInt64Type,
                (&mut out as *mut i64).cast::<c_void>(),
            )
        };
        return ok != 0 && out != 0;
    }
    false
}

/// Read a property from the service or any of its registry descendants.
fn descendant_property(service: io_service_t, key: &str) -> Option<CfOwned> {
    let cf_key = cf_string(key);
    let plane = CString::new("IOService").ok()?;
    let prop = unsafe {
        IORegistryEntrySearchCFProperty(
            service,
            plane.as_ptr(),
            cf_key.0,
            ptr::null(),
            K_IO_REGISTRY_ITERATE_RECURSIVELY,
        )
    };
    if prop.is_null() {
        None
    } else {
        Some(CfOwned(prop))
    }
}

fn property_string(service: io_service_t, key: &str) -> Option<String> {
    let prop = descendant_property(service, key)?;
    if unsafe { CFGetTypeID(prop.0) } != unsafe { CFStringGetTypeID() } {
        return None;
    }
    let mut buf = [0u8; 2048];
    let ok = unsafe {
        CFStringGetCString(
            prop.0,
            buf.as_mut_ptr().cast::<c_char>(),
            buf.len() as CFIndex,
            kCFStringEncodingUTF8,
        )
    };
    if ok == 0 {
        return None;
    }
    let cstr = unsafe { CStr::from_ptr(buf.as_ptr().cast::<c_char>()) };
    Some(cstr.to_string_lossy().into_owned())
}

fn property_u16(service: io_service_t, key: &str) -> Option<u16> {
    let prop = descendant_property(service, key)?;
    if unsafe { CFGetTypeID(prop.0) } != unsafe { CFNumberGetTypeID() } {
        return None;
    }
    let mut out: i64 = 0;
    let ok = unsafe {
        CFNumberGetValue(
            prop.0,
            kCFNumberSInt64Type,
            (&mut out as *mut i64).cast::<c_void>(),
        )
    };
    if ok == 0 {
        return None;
    }
    u16::try_from(out).ok()
}

/// The `fn` tables Apple's keyboard driver publishes for this device.
///
/// They live on the driver's event service, a registry descendant of the
/// `IOHIDDevice` we seize, so they are read before the device is opened.
fn read_fn_usage_map(service: io_service_t) -> FnUsageMap {
    FnUsageMap::from_parts(
        property_string(service, "FnFunctionUsageMap").as_deref(),
        property_string(service, "FnKeyboardUsageMap").as_deref(),
        property_u16(service, "FnModifierUsagePage"),
        property_u16(service, "FnModifierUsage"),
    )
}

fn is_karabiner_virtual_keyboard(device: IOHIDDeviceRef) -> bool {
    let vid = device_i64_prop(device, "VendorID").unwrap_or(-1);
    let pid = device_i64_prop(device, "ProductID").unwrap_or(-1);
    if vid == VIRTUAL_VENDOR_ID as i64 && pid == VIRTUAL_PRODUCT_ID as i64 {
        return true;
    }
    if let Some(m) = device_string_prop(device, "Manufacturer") {
        if m.eq_ignore_ascii_case("pqrs.org") {
            return true;
        }
    }
    if let Some(p) = device_string_prop(device, "Product") {
        if p.contains("Karabiner") || p.contains("VirtualHIDKeyboard") {
            return true;
        }
    }
    false
}

struct SeizeContext {
    remapper: Remapper,
    vhidd: VirtualKeyboardClient,
    /// Map of IOKit RegistryEntryID -> owned `IOHIDDeviceRef`.
    seized_devices: HashMap<u64, IOHIDDeviceRef>,
    /// Devices waiting for Input Monitoring permission to be granted.
    pending_devices: HashMap<u64, IOHIDDeviceRef>,
    /// `fn` tables per device, keyed by its `IOHIDDeviceRef`.
    fn_maps: HashMap<usize, FnUsageMap>,
    verbose: bool,
}

impl SeizeContext {
    fn emit_actions(&self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Send(report) => {
                    if let Err(e) = self.vhidd.post_report(report) {
                        eprintln!("[controlesque] failed to post HID report: {e}");
                    }
                }
                Action::TapEscape(base) => {
                    let (down, up) = escape_tap_reports(base);
                    if let Err(e) = self.vhidd.post_report(HidReport::Keyboard(down)) {
                        eprintln!("[controlesque] failed to post Escape down: {e}");
                    }
                    // Tiny separation so macOS HID event queue reliably registers press & release.
                    thread::sleep(Duration::from_millis(2));
                    if let Err(e) = self.vhidd.post_report(HidReport::Keyboard(up)) {
                        eprintln!("[controlesque] failed to post Escape up: {e}");
                    }
                }
            }
        }
    }
}

extern "C" fn on_service_matched(refcon: *mut c_void, iterator: io_iterator_t) {
    let ctx = unsafe { &mut *refcon.cast::<SeizeContext>() };

    loop {
        let service = unsafe { IOIteratorNext(iterator) };
        if service == 0 {
            break;
        }

        let mut reg_id: u64 = 0;
        unsafe { IORegistryEntryGetRegistryEntryID(service, &mut reg_id) };

        if ctx.seized_devices.contains_key(&reg_id) || ctx.pending_devices.contains_key(&reg_id) {
            unsafe { IOObjectRelease(service) };
            continue;
        }

        // Read the driver's `fn` tables before the service handle is released.
        let fn_map = read_fn_usage_map(service);
        let has_fn_tables = !fn_map.is_empty();
        let device = unsafe { IOHIDDeviceCreate(ptr::null(), service) };
        unsafe { IOObjectRelease(service) };
        if device.is_null() {
            continue;
        }

        let product = device_string_prop(device, "Product").unwrap_or_else(|| "<unknown>".into());
        let vid = device_i64_prop(device, "VendorID").unwrap_or(0);
        let pid = device_i64_prop(device, "ProductID").unwrap_or(0);

        if is_karabiner_virtual_keyboard(device) {
            if ctx.verbose {
                eprintln!(
                    "[controlesque] skipping virtual keyboard: {product} (0x{vid:04x}:0x{pid:04x})"
                );
            }
            unsafe { CFRelease(device) };
            continue;
        }

        ctx.fn_maps.insert(device as usize, fn_map);

        let kr = unsafe { IOHIDDeviceOpen(device, kIOHIDOptionsTypeSeizeDevice) };
        if kr == kIOReturnNotPermitted {
            eprintln!(
                "[controlesque] waiting for Input Monitoring permission to seize {product} \
                 (grant controlesque.app in System Settings -> Privacy & Security -> Input Monitoring)"
            );
            ctx.pending_devices.insert(reg_id, device);
            continue;
        }
        if kr != kIOReturnSuccess {
            eprintln!(
                "[controlesque] failed to seize {product} (0x{vid:04x}:0x{pid:04x}): IOReturn=0x{kr:08x}"
            );
            ctx.fn_maps.remove(&(device as usize));
            unsafe { CFRelease(device) };
            continue;
        }

        unsafe {
            IOHIDDeviceRegisterInputValueCallback(device, on_input_value, refcon);
            IOHIDDeviceScheduleWithRunLoop(device, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
        }

        ctx.seized_devices.insert(reg_id, device);
        eprintln!(
            "[controlesque] seized keyboard: {product} (0x{vid:04x}:0x{pid:04x}, id={reg_id}, \
             fn tables: {})",
            if has_fn_tables { "yes" } else { "none" }
        );
    }
}

extern "C" fn on_service_terminated(refcon: *mut c_void, iterator: io_iterator_t) {
    let ctx = unsafe { &mut *refcon.cast::<SeizeContext>() };

    loop {
        let service = unsafe { IOIteratorNext(iterator) };
        if service == 0 {
            break;
        }

        let mut reg_id: u64 = 0;
        unsafe { IORegistryEntryGetRegistryEntryID(service, &mut reg_id) };
        unsafe { IOObjectRelease(service) };

        if let Some(device) = ctx.pending_devices.remove(&reg_id) {
            ctx.fn_maps.remove(&(device as usize));
            unsafe { CFRelease(device) };
        }

        if let Some(device) = ctx.seized_devices.remove(&reg_id) {
            ctx.fn_maps.remove(&(device as usize));
            let product =
                device_string_prop(device, "Product").unwrap_or_else(|| "<unknown>".into());
            eprintln!("[controlesque] keyboard removed: {product} (id={reg_id})");

            unsafe {
                IOHIDDeviceUnscheduleFromRunLoop(
                    device,
                    CFRunLoopGetCurrent(),
                    kCFRunLoopDefaultMode,
                );
                IOHIDDeviceClose(device, kIOHIDOptionsTypeNone);
                CFRelease(device);
            }

            // Clear any keys that may have been held when the keyboard disconnected.
            for report in ctx.remapper.reset_all() {
                let _ = ctx.vhidd.post_report(report);
            }
        }
    }
}

extern "C" fn on_input_value(
    context: *mut c_void,
    _result: IOReturn,
    sender: *mut c_void,
    value: IOHIDValueRef,
) {
    // Guard against multi-byte vendor descriptor values that cannot fit in CFIndex.
    let len = unsafe { IOHIDValueGetLength(value) };
    if len <= 0 || len > std::mem::size_of::<CFIndex>() as CFIndex {
        return;
    }

    let elem = unsafe { IOHIDValueGetElement(value) };
    if elem.is_null() {
        return;
    }

    // Only process input elements (Misc=1, Button=2, Axis=3, ScanCodes=4).
    let elem_type = unsafe { IOHIDElementGetType(elem) };
    if !(1..=4).contains(&elem_type) {
        return;
    }

    let usage_page = unsafe { IOHIDElementGetUsagePage(elem) };
    let usage = unsafe { IOHIDElementGetUsage(elem) };
    let pressed = unsafe { IOHIDValueGetIntegerValue(value) != 0 };

    let ctx = unsafe { &mut *context.cast::<SeizeContext>() };
    let fallback = FnUsageMap::default();
    let map = ctx.fn_maps.get(&(sender as usize)).unwrap_or(&fallback);
    let actions = ctx
        .remapper
        .handle_event(usage_page, usage, pressed, Instant::now(), map);
    if !actions.is_empty() {
        ctx.emit_actions(actions);
    }
}

/// Run the IOKit HID seize loop until `running` becomes `false`.
pub fn run_seize_loop(
    vhidd: VirtualKeyboardClient,
    alone_timeout: Duration,
    running: &AtomicBool,
    verbose: bool,
) -> Result<(), String> {
    let standard_function_keys = use_fkeys_as_standard_function_keys();
    if verbose {
        eprintln!("[controlesque] use F1–F12 as standard function keys: {standard_function_keys}");
    }

    let mut ctx = Box::new(SeizeContext {
        remapper: Remapper::new(alone_timeout, standard_function_keys),
        vhidd,
        seized_devices: HashMap::new(),
        pending_devices: HashMap::new(),
        fn_maps: HashMap::new(),
        verbose,
    });
    let ctx_ptr = (&mut *ctx as *mut SeizeContext).cast::<c_void>();

    let notify_port = unsafe { IONotificationPortCreate(kIOMainPortDefault) };
    if notify_port.is_null() {
        return Err("IONotificationPortCreate returned null".into());
    }

    let run_loop = unsafe { CFRunLoopGetCurrent() };
    let run_loop_source = unsafe { IONotificationPortGetRunLoopSource(notify_port) };
    unsafe { CFRunLoopAddSource(run_loop, run_loop_source, kCFRunLoopDefaultMode) };

    let dict = make_keyboard_matching_dict();
    // IOServiceAddMatchingNotification consumes one reference per call, so retain once.
    unsafe { CFRetain(dict.cast::<c_void>()) };

    let matched_type = CString::new("IOServiceFirstMatch").unwrap();
    let terminated_type = CString::new("IOServiceTerminate").unwrap();

    let mut matched_iter: io_iterator_t = 0;
    let mut terminated_iter: io_iterator_t = 0;

    let kr_match = unsafe {
        IOServiceAddMatchingNotification(
            notify_port,
            matched_type.as_ptr(),
            dict.cast::<c_void>(),
            on_service_matched,
            ctx_ptr,
            &mut matched_iter,
        )
    };
    let kr_term = unsafe {
        IOServiceAddMatchingNotification(
            notify_port,
            terminated_type.as_ptr(),
            dict.cast::<c_void>(),
            on_service_terminated,
            ctx_ptr,
            &mut terminated_iter,
        )
    };

    if kr_match != kIOReturnSuccess || kr_term != kIOReturnSuccess {
        unsafe {
            CFRunLoopRemoveSource(run_loop, run_loop_source, kCFRunLoopDefaultMode);
            IONotificationPortDestroy(notify_port);
        }
        return Err(format!(
            "IOServiceAddMatchingNotification failed (match=0x{kr_match:08x}, term=0x{kr_term:08x})"
        ));
    }

    // Drain initial iterators to seize already-attached keyboards and arm hot-plug notifications.
    on_service_matched(ctx_ptr, matched_iter);
    on_service_terminated(ctx_ptr, terminated_iter);

    eprintln!(
        "[controlesque] running (protocol {:?}, tap timeout {}ms). Press Ctrl+C to stop.",
        ctx.vhidd.protocol(),
        alone_timeout.as_millis()
    );

    let mut last_retry = Instant::now();
    while running.load(Ordering::Relaxed) {
        // Sleep in the kernel run loop: 5s when idle (0.2 wakeups/s), 2s while awaiting permission.
        let timeout = if ctx.pending_devices.is_empty() {
            5.0
        } else {
            2.0
        };
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, timeout, 0);
        }
        // Automatically retry seizing devices once Input Monitoring permission is granted.
        if !ctx.pending_devices.is_empty() && last_retry.elapsed() >= Duration::from_secs(2) {
            last_retry = Instant::now();
            let pending_ids: Vec<u64> = ctx.pending_devices.keys().copied().collect();
            for reg_id in pending_ids {
                if let Some(&device) = ctx.pending_devices.get(&reg_id) {
                    let kr = unsafe { IOHIDDeviceOpen(device, kIOHIDOptionsTypeSeizeDevice) };
                    if kr == kIOReturnSuccess {
                        ctx.pending_devices.remove(&reg_id);
                        unsafe {
                            IOHIDDeviceRegisterInputValueCallback(device, on_input_value, ctx_ptr);
                            IOHIDDeviceScheduleWithRunLoop(device, run_loop, kCFRunLoopDefaultMode);
                        }
                        let product = device_string_prop(device, "Product")
                            .unwrap_or_else(|| "<unknown>".into());
                        ctx.seized_devices.insert(reg_id, device);
                        eprintln!("[controlesque] seized keyboard: {product} (id={reg_id})");
                    }
                }
            }
        }
    }

    eprintln!("[controlesque] shutting down, releasing keyboards...");

    // Release all virtual keys first so nothing is left stuck.
    for report in ctx.remapper.reset_all() {
        let _ = ctx.vhidd.post_report(report);
    }

    for (_, device) in ctx.pending_devices.drain() {
        unsafe { CFRelease(device) };
    }

    // Unseize every physical keyboard.
    for (_, device) in ctx.seized_devices.drain() {
        unsafe {
            IOHIDDeviceUnscheduleFromRunLoop(device, run_loop, kCFRunLoopDefaultMode);
            IOHIDDeviceClose(device, kIOHIDOptionsTypeNone);
            CFRelease(device);
        }
    }

    unsafe {
        IOObjectRelease(matched_iter);
        IOObjectRelease(terminated_iter);
        CFRunLoopRemoveSource(run_loop, run_loop_source, kCFRunLoopDefaultMode);
        IONotificationPortDestroy(notify_port);
    }

    Ok(())
}
