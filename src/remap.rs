//! Caps Lock dual-role remapper state machine:
//! - Tap Caps Lock alone (within `alone_timeout`) -> Escape
//! - Hold Caps Lock (with another key, or longer than `alone_timeout`) -> Left Control
//!
//! Matches Karabiner-Elements' `to: [left_control], to_if_alone: [escape]` behavior:
//! Left Control is asserted immediately on Caps Lock press so chords (`Caps+C`, `Caps+[`)
//! and `Caps+Click` with a trackpad/mouse have zero latency.

use crate::fnmap::{FnUsageMap, PackedUsage, pack};
use crate::hid::{
    APPLE_TOP_CASE_BRIGHTNESS_DOWN, APPLE_TOP_CASE_BRIGHTNESS_UP, AuxiliaryReport,
    CONSUMER_DISPLAY_BRIGHTNESS_DECREMENT, CONSUMER_DISPLAY_BRIGHTNESS_INCREMENT, HidReport,
    KEY_CAPS_LOCK, KEY_ESCAPE, KeyboardReport, MOD_LEFT_CTRL, PAGE_APPLE_VENDOR_KEYBOARD,
    PAGE_APPLE_VENDOR_TOP_CASE, PAGE_CONSUMER, PAGE_GENERIC_DESKTOP, PAGE_KEYBOARD, modifier_bit,
};
use std::time::{Duration, Instant};

/// Output action emitted by the remapper.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Post a HID report immediately.
    Send(HidReport),
    /// Tap Escape (press + brief pause + release) using the given base report.
    TapEscape(KeyboardReport),
}

#[derive(Clone, Copy, Debug)]
struct CapsState {
    pressed_at: Instant,
    used_with_other_key: bool,
}

pub struct Remapper {
    alone_timeout: Duration,
    /// Value of macOS' "Use F1, F2, etc. keys as standard function keys".
    standard_function_keys: bool,
    caps: Option<CapsState>,
    physical_modifiers: u8,
    keyboard: KeyboardReport,
    consumer: AuxiliaryReport,
    apple_top_case: AuxiliaryReport,
    apple_keyboard: AuxiliaryReport,
    generic_desktop: AuxiliaryReport,
    /// Translations currently emitted, keyed by the physical usage. Release
    /// always undoes exactly what press produced, even if `fn` changes between.
    translated: Vec<(PackedUsage, PackedUsage)>,
}

impl Remapper {
    pub fn new(alone_timeout: Duration, standard_function_keys: bool) -> Self {
        Self {
            alone_timeout,
            standard_function_keys,
            caps: None,
            physical_modifiers: 0,
            keyboard: KeyboardReport::default(),
            consumer: AuxiliaryReport::default(),
            apple_top_case: AuxiliaryReport::default(),
            apple_keyboard: AuxiliaryReport::default(),
            generic_desktop: AuxiliaryReport::default(),
            translated: Vec::new(),
        }
    }

    fn effective_modifiers(&self) -> u8 {
        if self.caps.is_some() {
            self.physical_modifiers | MOD_LEFT_CTRL
        } else {
            self.physical_modifiers
        }
    }

    /// Process a single raw HID input event from a seized physical keyboard.
    ///
    /// `map` carries the `fn` tables published by that keyboard's macOS driver.
    pub fn handle_event(
        &mut self,
        usage_page: u32,
        usage: u32,
        pressed: bool,
        now: Instant,
        map: &FnUsageMap,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        if usage == 0 || usage > u16::MAX as u32 {
            return actions;
        }
        let usage_u16 = usage as u16;

        match usage_page {
            PAGE_KEYBOARD => {
                // Ignore HID error/phantom usages (0x01 ErrorRollOver, 0x02 POSTFail, 0x03 ErrorUndefined)
                // and anything above standard Right GUI (0xE7).
                if !(0x04..=0xE7).contains(&usage_u16) {
                    return actions;
                }

                if usage_u16 == KEY_CAPS_LOCK {
                    self.handle_caps_lock(pressed, now, &mut actions);
                    return actions;
                }

                // Any other key press while Caps Lock is down turns it into pure Ctrl.
                if pressed {
                    if let Some(ref mut caps) = self.caps {
                        caps.used_with_other_key = true;
                    }
                }

                // The keyboard's own driver tables describe what `fn` chords and
                // the top row mean. Seizing the device bypasses that driver
                // (which is why the top row arrives as plain F-keys), so replay
                // its tables here instead of assuming a particular layout.
                let source = pack(PAGE_KEYBOARD, usage);

                // A key that one of the driver tables owns is translated on
                // press and always released through the same path, so a change
                // of `fn` state mid-hold cannot leave it stuck.
                let translatable = map.function(source).is_some() || map.keyboard(source).is_some();

                if translatable {
                    if !pressed {
                        if !self.release_translated(source, &mut actions)
                            && self.keyboard.keys.erase(usage_u16)
                        {
                            actions.push(Action::Send(HidReport::Keyboard(self.keyboard)));
                        }
                        return actions;
                    }

                    // `fn` + key: Home/End/Page Up/Page Down/Forward Delete.
                    if self.fn_held(map)
                        && let Some(target) = map.keyboard(source)
                    {
                        self.press_translated(source, target, &mut actions);
                        return actions;
                    }

                    // Top row: media key, or plain F-key, depending on the
                    // "standard function keys" setting (which inverts `fn`).
                    if let Some(target) = map.function(source) {
                        if self.standard_function_keys == self.fn_held(map) {
                            self.press_translated(source, target, &mut actions);
                        } else if self.keyboard.keys.insert(usage_u16) {
                            actions.push(Action::Send(HidReport::Keyboard(self.keyboard)));
                        }
                        return actions;
                    }
                }

                let changed = if let Some(bit) = modifier_bit(usage_u16) {
                    if pressed {
                        self.physical_modifiers |= bit;
                    } else {
                        self.physical_modifiers &= !bit;
                    }
                    let new_eff = self.effective_modifiers();
                    let diff = new_eff != self.keyboard.modifiers;
                    self.keyboard.modifiers = new_eff;
                    diff
                } else if pressed {
                    self.keyboard.keys.insert(usage_u16)
                } else {
                    self.keyboard.keys.erase(usage_u16)
                };

                if changed {
                    actions.push(Action::Send(HidReport::Keyboard(self.keyboard)));
                }
            }
            PAGE_CONSUMER => {
                if pressed {
                    if let Some(ref mut caps) = self.caps {
                        caps.used_with_other_key = true;
                    }
                }
                let changed = if pressed {
                    self.consumer.keys.insert(usage_u16)
                } else {
                    self.consumer.keys.erase(usage_u16)
                };
                if changed {
                    actions.push(Action::Send(HidReport::Consumer(self.consumer)));
                }
            }
            PAGE_APPLE_VENDOR_TOP_CASE => {
                if pressed {
                    if let Some(ref mut caps) = self.caps {
                        caps.used_with_other_key = true;
                    }
                }
                let changed = if pressed {
                    self.apple_top_case.keys.insert(usage_u16)
                } else {
                    self.apple_top_case.keys.erase(usage_u16)
                };
                if changed {
                    actions.push(Action::Send(HidReport::AppleVendorTopCase(
                        self.apple_top_case,
                    )));
                }
            }
            PAGE_APPLE_VENDOR_KEYBOARD => {
                if pressed {
                    if let Some(ref mut caps) = self.caps {
                        caps.used_with_other_key = true;
                    }
                }
                let changed = if pressed {
                    self.apple_keyboard.keys.insert(usage_u16)
                } else {
                    self.apple_keyboard.keys.erase(usage_u16)
                };
                if changed {
                    actions.push(Action::Send(HidReport::AppleVendorKeyboard(
                        self.apple_keyboard,
                    )));
                }
            }
            PAGE_GENERIC_DESKTOP => {
                // Only forward system power/sleep/wake usages (0x81..=0x83).
                if !(0x81..=0x83).contains(&usage_u16) {
                    return actions;
                }
                let changed = if pressed {
                    self.generic_desktop.keys.insert(usage_u16)
                } else {
                    self.generic_desktop.keys.erase(usage_u16)
                };
                if changed {
                    actions.push(Action::Send(HidReport::GenericDesktop(
                        self.generic_desktop,
                    )));
                }
            }
            _ => {}
        }

        actions
    }

    /// Emit a driver-table translation and remember it for its release.
    fn press_translated(
        &mut self,
        source: PackedUsage,
        target: PackedUsage,
        actions: &mut Vec<Action>,
    ) {
        // Ignore key-repeat / duplicate down events.
        if self.translated.iter().any(|(held, _)| *held == source) {
            return;
        }
        self.translated.push((source, target));
        self.emit_usage(target, true, actions);
    }

    /// Release the translation recorded for `source`; false if there is none.
    fn release_translated(&mut self, source: PackedUsage, actions: &mut Vec<Action>) -> bool {
        let Some(index) = self.translated.iter().position(|(held, _)| *held == source) else {
            return false;
        };
        let (_, target) = self.translated.remove(index);
        self.emit_usage(target, false, actions);
        true
    }

    fn fn_held(&self, map: &FnUsageMap) -> bool {
        map.fn_page() == PAGE_APPLE_VENDOR_TOP_CASE as u16
            && self.apple_top_case.keys.contains(map.fn_usage())
    }

    /// Press or release a packed `(page, usage)` on the report that page lives in.
    fn emit_usage(&mut self, packed: PackedUsage, pressed: bool, actions: &mut Vec<Action>) {
        // Apple's driver maps F1/F2 to AppleVendorTopCase brightness usages, but
        // macOS ignores those from a virtual keyboard. Post the consumer-page
        // equivalents instead, as Karabiner does.
        let packed = match packed {
            p if p
                == pack(
                    PAGE_APPLE_VENDOR_TOP_CASE,
                    APPLE_TOP_CASE_BRIGHTNESS_UP as u32,
                ) =>
            {
                pack(PAGE_CONSUMER, CONSUMER_DISPLAY_BRIGHTNESS_INCREMENT as u32)
            }
            p if p
                == pack(
                    PAGE_APPLE_VENDOR_TOP_CASE,
                    APPLE_TOP_CASE_BRIGHTNESS_DOWN as u32,
                ) =>
            {
                pack(PAGE_CONSUMER, CONSUMER_DISPLAY_BRIGHTNESS_DECREMENT as u32)
            }
            other => other,
        };

        let page = (packed >> 16) as u32;
        let usage = (packed & 0xFFFF) as u16;

        let (changed, report) = match page {
            PAGE_KEYBOARD => (
                if pressed {
                    self.keyboard.keys.insert(usage)
                } else {
                    self.keyboard.keys.erase(usage)
                },
                HidReport::Keyboard(self.keyboard),
            ),
            PAGE_CONSUMER => (
                if pressed {
                    self.consumer.keys.insert(usage)
                } else {
                    self.consumer.keys.erase(usage)
                },
                HidReport::Consumer(self.consumer),
            ),
            PAGE_APPLE_VENDOR_TOP_CASE => (
                if pressed {
                    self.apple_top_case.keys.insert(usage)
                } else {
                    self.apple_top_case.keys.erase(usage)
                },
                HidReport::AppleVendorTopCase(self.apple_top_case),
            ),
            PAGE_APPLE_VENDOR_KEYBOARD => (
                if pressed {
                    self.apple_keyboard.keys.insert(usage)
                } else {
                    self.apple_keyboard.keys.erase(usage)
                },
                HidReport::AppleVendorKeyboard(self.apple_keyboard),
            ),
            PAGE_GENERIC_DESKTOP => (
                if pressed {
                    self.generic_desktop.keys.insert(usage)
                } else {
                    self.generic_desktop.keys.erase(usage)
                },
                HidReport::GenericDesktop(self.generic_desktop),
            ),
            // Unknown destination page: drop rather than fall back to the raw
            // key, which would defeat the translation entirely.
            _ => return,
        };

        if changed {
            actions.push(Action::Send(report));
        }
    }

    fn handle_caps_lock(&mut self, pressed: bool, now: Instant, actions: &mut Vec<Action>) {
        if pressed {
            if self.caps.is_some() {
                // Ignore key-repeat / duplicate down events while already held.
                return;
            }
            self.caps = Some(CapsState {
                pressed_at: now,
                used_with_other_key: false,
            });
            let new_eff = self.effective_modifiers();
            if new_eff != self.keyboard.modifiers {
                self.keyboard.modifiers = new_eff;
                actions.push(Action::Send(HidReport::Keyboard(self.keyboard)));
            }
        } else if let Some(state) = self.caps.take() {
            let new_eff = self.effective_modifiers();
            if new_eff != self.keyboard.modifiers {
                self.keyboard.modifiers = new_eff;
                actions.push(Action::Send(HidReport::Keyboard(self.keyboard)));
            }

            let elapsed = now.saturating_duration_since(state.pressed_at);
            if !state.used_with_other_key && elapsed <= self.alone_timeout {
                actions.push(Action::TapEscape(self.keyboard));
            }
        }
    }

    /// Release all held keys (used on device disconnect or daemon shutdown).
    pub fn reset_all(&mut self) -> Vec<HidReport> {
        self.caps = None;
        self.physical_modifiers = 0;
        self.keyboard = KeyboardReport::default();
        self.translated.clear();
        self.consumer.keys.clear();
        self.apple_top_case.keys.clear();
        self.apple_keyboard.keys.clear();
        self.generic_desktop.keys.clear();

        vec![
            HidReport::Keyboard(self.keyboard),
            HidReport::Consumer(self.consumer),
            HidReport::AppleVendorTopCase(self.apple_top_case),
            HidReport::AppleVendorKeyboard(self.apple_keyboard),
            HidReport::GenericDesktop(self.generic_desktop),
        ]
    }
}

/// Build the press & release keyboard reports for an Escape tap.
pub fn escape_tap_reports(base: KeyboardReport) -> (KeyboardReport, KeyboardReport) {
    let mut down = base;
    down.keys.insert(KEY_ESCAPE);
    let mut up = base;
    up.keys.erase(KEY_ESCAPE);
    (down, up)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid::KEY_LEFT_CTRL;

    const KEY_C: u32 = 0x06;

    /// The tables this Mac's built-in keyboard publishes.
    const FUNCTIONS: &str = "0x0007003a,0x00ff0005,0x0007003b,0x00ff0004,0x0007003c,0xff010010,0x0007003d,0x000c0221,0x0007003e,0x000c00cf,0x0007003f,0x0001009b,0x00070040,0x000c00b4,0x00070041,0x000c00cd,0x00070042,0x000c00b3,0x00070043,0x000c00e2,0x00070044,0x000c00ea,0x00070045,0x000c00e9";
    const FN_KEYBOARD: &str = "0x00070050,0x0007004a,0x00070052,0x0007004b,0x0007002a,0x0007004c,0x0007004f,0x0007004d,0x00070051,0x0007004e,0x00070028,0x00070058";

    const F2: u32 = 0x3B;
    const F3: u32 = 0x3C;
    const F11: u32 = 0x44;
    const LEFT_ARROW: u32 = 0x50;
    const FN: u32 = 0x03;
    const HOME: u16 = 0x4A;

    fn apple_map() -> FnUsageMap {
        FnUsageMap::from_parts(Some(FUNCTIONS), Some(FN_KEYBOARD), Some(0xFF), Some(3))
    }

    fn press_fn(r: &mut Remapper, t: Instant) {
        r.handle_event(PAGE_APPLE_VENDOR_TOP_CASE, FN, true, t, &apple_map());
    }

    #[test]
    fn quick_tap_emits_escape() {
        let mut r = Remapper::new(Duration::from_millis(500), false);
        let t0 = Instant::now();

        let down = r.handle_event(
            PAGE_KEYBOARD,
            KEY_CAPS_LOCK as u32,
            true,
            t0,
            &FnUsageMap::default(),
        );
        assert_eq!(down.len(), 1);
        match down[0] {
            Action::Send(HidReport::Keyboard(kb)) => assert_eq!(kb.modifiers, MOD_LEFT_CTRL),
            _ => panic!("expected left_control down"),
        }

        let up = r.handle_event(
            PAGE_KEYBOARD,
            KEY_CAPS_LOCK as u32,
            false,
            t0 + Duration::from_millis(80),
            &FnUsageMap::default(),
        );
        assert_eq!(up.len(), 2);
        match up[0] {
            Action::Send(HidReport::Keyboard(kb)) => assert_eq!(kb.modifiers, 0),
            _ => panic!("expected left_control up"),
        }
        assert!(matches!(up[1], Action::TapEscape(_)));
    }

    #[test]
    fn hold_past_timeout_does_not_emit_escape() {
        let mut r = Remapper::new(Duration::from_millis(500), false);
        let t0 = Instant::now();

        r.handle_event(
            PAGE_KEYBOARD,
            KEY_CAPS_LOCK as u32,
            true,
            t0,
            &FnUsageMap::default(),
        );
        let up = r.handle_event(
            PAGE_KEYBOARD,
            KEY_CAPS_LOCK as u32,
            false,
            t0 + Duration::from_millis(600),
            &FnUsageMap::default(),
        );
        assert_eq!(up.len(), 1);
        assert!(matches!(up[0], Action::Send(HidReport::Keyboard(_))));
    }

    #[test]
    fn chord_with_other_key_is_ctrl_only() {
        let mut r = Remapper::new(Duration::from_millis(500), false);
        let t0 = Instant::now();

        r.handle_event(
            PAGE_KEYBOARD,
            KEY_CAPS_LOCK as u32,
            true,
            t0,
            &FnUsageMap::default(),
        );

        let c_down = r.handle_event(
            PAGE_KEYBOARD,
            KEY_C,
            true,
            t0 + Duration::from_millis(20),
            &FnUsageMap::default(),
        );
        match c_down[0] {
            Action::Send(HidReport::Keyboard(kb)) => {
                assert_eq!(kb.modifiers, MOD_LEFT_CTRL);
                assert!(kb.keys.contains(KEY_C as u16));
            }
            _ => panic!("expected Ctrl+C down"),
        }

        r.handle_event(
            PAGE_KEYBOARD,
            KEY_C,
            false,
            t0 + Duration::from_millis(50),
            &FnUsageMap::default(),
        );

        let caps_up = r.handle_event(
            PAGE_KEYBOARD,
            KEY_CAPS_LOCK as u32,
            false,
            t0 + Duration::from_millis(80),
            &FnUsageMap::default(),
        );
        // Only Left Ctrl up; NO Escape tap!
        assert_eq!(caps_up.len(), 1);
        match caps_up[0] {
            Action::Send(HidReport::Keyboard(kb)) => assert_eq!(kb.modifiers, 0),
            _ => panic!("expected left_control up"),
        }
    }

    #[test]
    fn physical_left_ctrl_preserved_when_caps_released() {
        let mut r = Remapper::new(Duration::from_millis(500), false);
        let t0 = Instant::now();

        // Hold physical Left Control, then press & release Caps Lock.
        r.handle_event(
            PAGE_KEYBOARD,
            KEY_LEFT_CTRL as u32,
            true,
            t0,
            &FnUsageMap::default(),
        );
        let caps_down = r.handle_event(
            PAGE_KEYBOARD,
            KEY_CAPS_LOCK as u32,
            true,
            t0 + Duration::from_millis(10),
            &FnUsageMap::default(),
        );
        // Left Control is already set, so no redundant report needed.
        assert!(caps_down.is_empty());

        let caps_up = r.handle_event(
            PAGE_KEYBOARD,
            KEY_CAPS_LOCK as u32,
            false,
            t0 + Duration::from_millis(40),
            &FnUsageMap::default(),
        );
        // Physical Left Control is still held!
        assert_eq!(r.effective_modifiers(), MOD_LEFT_CTRL);
        assert!(matches!(caps_up.last(), Some(Action::TapEscape(_))));
    }

    #[test]
    fn top_row_follows_the_keyboards_driver_table() {
        let mut r = Remapper::new(Duration::from_millis(500), false);
        let t0 = Instant::now();

        // This Mac maps F2 to AppleVendorTopCase brightness up, which is posted
        // as the consumer-page brightness key (what a virtual keyboard accepts).
        let down = r.handle_event(PAGE_KEYBOARD, F2, true, t0, &apple_map());
        match down.as_slice() {
            [Action::Send(HidReport::Consumer(report))] => {
                assert!(report.keys.contains(CONSUMER_DISPLAY_BRIGHTNESS_INCREMENT));
            }
            other => panic!("expected brightness up, got {other:?}"),
        }

        let up = r.handle_event(PAGE_KEYBOARD, F2, false, t0, &apple_map());
        match up.as_slice() {
            [Action::Send(HidReport::Consumer(report))] => {
                assert!(!report.keys.contains(CONSUMER_DISPLAY_BRIGHTNESS_INCREMENT));
            }
            other => panic!("expected brightness up release, got {other:?}"),
        }
    }

    #[test]
    fn top_row_follows_the_driver_table_across_pages() {
        let mut r = Remapper::new(Duration::from_millis(500), false);
        let t0 = Instant::now();

        // F3 -> AppleVendorKeyboard expose_all (Mission Control).
        let mission_control = r.handle_event(PAGE_KEYBOARD, F3, true, t0, &apple_map());
        match mission_control.as_slice() {
            [Action::Send(HidReport::AppleVendorKeyboard(report))] => {
                assert!(report.keys.contains(0x0010));
            }
            other => panic!("expected Mission Control, got {other:?}"),
        }

        // F11 -> Consumer volume decrement.
        let volume = r.handle_event(PAGE_KEYBOARD, F11, true, t0, &apple_map());
        match volume.as_slice() {
            [Action::Send(HidReport::Consumer(report))] => {
                assert!(report.keys.contains(0x00EA));
            }
            other => panic!("expected volume down, got {other:?}"),
        }
    }

    #[test]
    fn top_row_is_raw_while_fn_is_held() {
        let mut r = Remapper::new(Duration::from_millis(500), false);
        let t0 = Instant::now();
        press_fn(&mut r, t0);

        let down = r.handle_event(PAGE_KEYBOARD, F2, true, t0, &apple_map());
        match down.as_slice() {
            [Action::Send(HidReport::Keyboard(report))] => assert!(report.keys.contains(F2 as u16)),
            other => panic!("expected raw F2, got {other:?}"),
        }
    }

    #[test]
    fn top_row_is_raw_when_standard_function_keys_is_enabled() {
        let mut r = Remapper::new(Duration::from_millis(500), true);
        let down = r.handle_event(PAGE_KEYBOARD, F2, true, Instant::now(), &apple_map());
        match down.as_slice() {
            [Action::Send(HidReport::Keyboard(report))] => assert!(report.keys.contains(F2 as u16)),
            other => panic!("expected raw F2, got {other:?}"),
        }
    }

    #[test]
    fn top_row_media_applies_to_fn_when_standard_function_keys_is_enabled() {
        let mut r = Remapper::new(Duration::from_millis(500), true);
        let t0 = Instant::now();
        press_fn(&mut r, t0);

        let down = r.handle_event(PAGE_KEYBOARD, F2, true, t0, &apple_map());
        match down.as_slice() {
            [Action::Send(HidReport::Consumer(report))] => {
                assert!(report.keys.contains(CONSUMER_DISPLAY_BRIGHTNESS_INCREMENT));
            }
            other => panic!("expected brightness up, got {other:?}"),
        }
    }

    #[test]
    fn keyboard_without_a_driver_table_passes_f_keys_through() {
        let mut r = Remapper::new(Duration::from_millis(500), false);
        let down = r.handle_event(
            PAGE_KEYBOARD,
            F2,
            true,
            Instant::now(),
            &FnUsageMap::default(),
        );
        match down.as_slice() {
            [Action::Send(HidReport::Keyboard(report))] => assert!(report.keys.contains(F2 as u16)),
            other => panic!("expected raw F2, got {other:?}"),
        }
    }

    #[test]
    fn fn_arrow_follows_the_driver_table() {
        let mut r = Remapper::new(Duration::from_millis(500), false);
        let t0 = Instant::now();
        press_fn(&mut r, t0);

        let down = r.handle_event(PAGE_KEYBOARD, LEFT_ARROW, true, t0, &apple_map());
        match down.as_slice() {
            [Action::Send(HidReport::Keyboard(report))] => assert!(report.keys.contains(HOME)),
            other => panic!("expected Home, got {other:?}"),
        }
    }

    #[test]
    fn top_row_release_undoes_the_press_even_if_fn_changes_in_between() {
        let mut r = Remapper::new(Duration::from_millis(500), false);
        let t0 = Instant::now();

        r.handle_event(PAGE_KEYBOARD, F11, true, t0, &apple_map());
        press_fn(&mut r, t0);

        let up = r.handle_event(PAGE_KEYBOARD, F11, false, t0, &apple_map());
        match up.as_slice() {
            [Action::Send(HidReport::Consumer(report))] => {
                assert!(!report.keys.contains(0x00EA));
            }
            other => panic!("expected volume down release, got {other:?}"),
        }
    }
}
