//! macOS IOKit `IOHIDManager` physical keyboard seizer.
//!
//! Uses the exact same OS-level mechanism as Karabiner-Elements:
//! `IOHIDDeviceOpen(device, kIOHIDOptionsTypeSeizeDevice)` claims physical keyboards
//! exclusively at the IOKit HID layer (below WindowServer, works in Secure Input),
//! while skipping Karabiner's own DriverKit virtual keyboard to avoid feedback loops.

#![allow(non_camel_case_types, non_upper_case_globals)]

use crate::hid::HidReport;
use crate::remap::{Action, Remapper, escape_tap_reports};
use crate::vhidd::{VIRTUAL_PRODUCT_ID, VIRTUAL_VENDOR_ID, VirtualKeyboardClient};
use std::collections::HashSet;
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
type CFIndex = isize;
type CFTypeID = usize;
type CFStringEncoding = u32;
type CFNumberType = isize;
type CFTimeInterval = f64;
type Boolean = u8;

type IOReturn = i32;
type IOOptionBits = u32;
type IOHIDManagerRef = *mut c_void;
type IOHIDDeviceRef = *mut c_void;
type IOHIDValueRef = *const c_void;
type IOHIDElementRef = *const c_void;

type IOHIDDeviceCallback = extern "C" fn(
    context: *mut c_void,
    result: IOReturn,
    sender: *mut c_void,
    device: IOHIDDeviceRef,
);

type IOHIDValueCallback = extern "C" fn(
    context: *mut c_void,
    result: IOReturn,
    sender: *mut c_void,
    value: IOHIDValueRef,
);

const kCFStringEncodingUTF8: CFStringEncoding = 0x0800_0100;
const kCFNumberSInt32Type: CFNumberType = 3;
const kCFNumberSInt64Type: CFNumberType = 4;

const kIOReturnSuccess: IOReturn = 0;
const kIOHIDOptionsTypeNone: IOOptionBits = 0;
const kIOHIDOptionsTypeSeizeDevice: IOOptionBits = 1;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: CFStringRef;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;

    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
    fn CFStringGetTypeID() -> CFTypeID;
    fn CFNumberGetTypeID() -> CFTypeID;

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

    fn CFDictionaryCreateMutable(
        allocator: CFAllocatorRef,
        capacity: CFIndex,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFMutableDictionaryRef;

    fn CFDictionarySetValue(
        the_dict: CFMutableDictionaryRef,
        key: *const c_void,
        value: *const c_void,
    );

    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopRunInMode(
        mode: CFStringRef,
        seconds: CFTimeInterval,
        return_after_source_handled: Boolean,
    ) -> i32;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDManagerCreate(allocator: CFAllocatorRef, options: IOOptionBits) -> IOHIDManagerRef;
    fn IOHIDManagerSetDeviceMatching(manager: IOHIDManagerRef, matching: CFDictionaryRef);
    fn IOHIDManagerRegisterDeviceMatchingCallback(
        manager: IOHIDManagerRef,
        callback: IOHIDDeviceCallback,
        context: *mut c_void,
    );
    fn IOHIDManagerRegisterDeviceRemovalCallback(
        manager: IOHIDManagerRef,
        callback: IOHIDDeviceCallback,
        context: *mut c_void,
    );
    fn IOHIDManagerScheduleWithRunLoop(
        manager: IOHIDManagerRef,
        run_loop: CFRunLoopRef,
        run_loop_mode: CFStringRef,
    );
    fn IOHIDManagerUnscheduleFromRunLoop(
        manager: IOHIDManagerRef,
        run_loop: CFRunLoopRef,
        run_loop_mode: CFStringRef,
    );
    fn IOHIDManagerOpen(manager: IOHIDManagerRef, options: IOOptionBits) -> IOReturn;
    fn IOHIDManagerClose(manager: IOHIDManagerRef, options: IOOptionBits) -> IOReturn;

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
    seized_devices: HashSet<usize>,
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

extern "C" fn on_device_matched(
    context: *mut c_void,
    _result: IOReturn,
    _sender: *mut c_void,
    device: IOHIDDeviceRef,
) {
    let ctx = unsafe { &mut *context.cast::<SeizeContext>() };
    let key = device as usize;
    if ctx.seized_devices.contains(&key) {
        return;
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
        return;
    }

    let kr = unsafe { IOHIDDeviceOpen(device, kIOHIDOptionsTypeSeizeDevice) };
    if kr != kIOReturnSuccess {
        eprintln!(
            "[controlesque] failed to seize {product} (0x{vid:04x}:0x{pid:04x}): IOReturn=0x{kr:08x}"
        );
        return;
    }

    unsafe {
        IOHIDDeviceRegisterInputValueCallback(device, on_input_value, context);
        IOHIDDeviceScheduleWithRunLoop(device, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
    }

    ctx.seized_devices.insert(key);
    eprintln!("[controlesque] seized keyboard: {product} (0x{vid:04x}:0x{pid:04x})");
}

extern "C" fn on_device_removed(
    context: *mut c_void,
    _result: IOReturn,
    _sender: *mut c_void,
    device: IOHIDDeviceRef,
) {
    let ctx = unsafe { &mut *context.cast::<SeizeContext>() };
    let key = device as usize;
    if !ctx.seized_devices.remove(&key) {
        return;
    }

    let product = device_string_prop(device, "Product").unwrap_or_else(|| "<unknown>".into());
    eprintln!("[controlesque] keyboard removed: {product}");

    unsafe {
        IOHIDDeviceUnscheduleFromRunLoop(device, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
        IOHIDDeviceClose(device, kIOHIDOptionsTypeNone);
    }

    // Clear any keys that may have been held when the keyboard disconnected.
    for report in ctx.remapper.reset_all() {
        let _ = ctx.vhidd.post_report(report);
    }
}

extern "C" fn on_input_value(
    context: *mut c_void,
    _result: IOReturn,
    _sender: *mut c_void,
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
    let actions = ctx
        .remapper
        .handle_event(usage_page, usage, pressed, Instant::now());
    if !actions.is_empty() {
        ctx.emit_actions(actions);
    }
}

/// Run the IOKit HID run loop until `running` becomes `false`.
pub fn run_seize_loop(
    vhidd: VirtualKeyboardClient,
    alone_timeout: Duration,
    running: &AtomicBool,
    verbose: bool,
) -> Result<(), String> {
    let mut ctx = Box::new(SeizeContext {
        remapper: Remapper::new(alone_timeout),
        vhidd,
        seized_devices: HashSet::new(),
        verbose,
    });
    let ctx_ptr = (&mut *ctx as *mut SeizeContext).cast::<c_void>();

    let manager = unsafe { IOHIDManagerCreate(ptr::null(), kIOHIDOptionsTypeNone) };
    if manager.is_null() {
        return Err("IOHIDManagerCreate returned null".into());
    }

    // Match Generic Desktop (0x01) / Keyboard (0x06) devices.
    let dict = unsafe {
        CFDictionaryCreateMutable(
            ptr::null(),
            2,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    };
    let page_key = cf_string("DeviceUsagePage");
    let page_val = cf_number_i32(0x01);
    let usage_key = cf_string("DeviceUsage");
    let usage_val = cf_number_i32(0x06);
    unsafe {
        CFDictionarySetValue(dict, page_key.0, page_val.0);
        CFDictionarySetValue(dict, usage_key.0, usage_val.0);
        IOHIDManagerSetDeviceMatching(manager, dict);
        CFRelease(dict);
    }

    unsafe {
        IOHIDManagerRegisterDeviceMatchingCallback(manager, on_device_matched, ctx_ptr);
        IOHIDManagerRegisterDeviceRemovalCallback(manager, on_device_removed, ctx_ptr);
        IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
    }

    let kr = unsafe { IOHIDManagerOpen(manager, kIOHIDOptionsTypeNone) };
    if kr != kIOReturnSuccess {
        unsafe { CFRelease(manager) };
        return Err(format!(
            "IOHIDManagerOpen failed (IOReturn=0x{kr:08x}). \
             Grant Input Monitoring permission and run with sudo."
        ));
    }

    eprintln!(
        "[controlesque] running (protocol {:?}, tap timeout {}ms). Press Ctrl+C to stop.",
        ctx.vhidd.protocol(),
        alone_timeout.as_millis()
    );

    while running.load(Ordering::Relaxed) {
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, 0);
        }
    }

    eprintln!("[controlesque] shutting down, releasing keyboards...");

    // Release all virtual keys first so nothing is left stuck.
    for report in ctx.remapper.reset_all() {
        let _ = ctx.vhidd.post_report(report);
    }

    // Unseize every physical keyboard.
    for device_addr in ctx.seized_devices.drain() {
        let device = device_addr as IOHIDDeviceRef;
        unsafe {
            IOHIDDeviceUnscheduleFromRunLoop(device, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
            IOHIDDeviceClose(device, kIOHIDOptionsTypeNone);
        }
    }

    unsafe {
        IOHIDManagerUnscheduleFromRunLoop(manager, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
        IOHIDManagerClose(manager, kIOHIDOptionsTypeNone);
        CFRelease(manager);
    }

    Ok(())
}
