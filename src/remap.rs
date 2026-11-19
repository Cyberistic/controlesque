//! Caps Lock dual-role remapper state machine:
//! - Tap Caps Lock alone (within `alone_timeout`) -> Escape
//! - Hold Caps Lock (with another key, or longer than `alone_timeout`) -> Left Control
//!
//! Matches Karabiner-Elements' `to: [left_control], to_if_alone: [escape]` behavior:
//! Left Control is asserted immediately on Caps Lock press so chords (`Caps+C`, `Caps+[`)
//! and `Caps+Click` with a trackpad/mouse have zero latency.

use crate::hid::{
    AuxiliaryReport, HidReport, KEY_CAPS_LOCK, KEY_ESCAPE, KeyboardReport, MOD_LEFT_CTRL,
    PAGE_APPLE_VENDOR_KEYBOARD, PAGE_APPLE_VENDOR_TOP_CASE, PAGE_CONSUMER, PAGE_GENERIC_DESKTOP,
    PAGE_KEYBOARD, modifier_bit,
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
    caps: Option<CapsState>,
    physical_modifiers: u8,
    keyboard: KeyboardReport,
    consumer: AuxiliaryReport,
    apple_top_case: AuxiliaryReport,
    apple_keyboard: AuxiliaryReport,
    generic_desktop: AuxiliaryReport,
}

impl Remapper {
    pub fn new(alone_timeout: Duration) -> Self {
        Self {
            alone_timeout,
            caps: None,
            physical_modifiers: 0,
            keyboard: KeyboardReport::default(),
            consumer: AuxiliaryReport::default(),
            apple_top_case: AuxiliaryReport::default(),
            apple_keyboard: AuxiliaryReport::default(),
            generic_desktop: AuxiliaryReport::default(),
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
    pub fn handle_event(
        &mut self,
        usage_page: u32,
        usage: u32,
        pressed: bool,
        now: Instant,
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

                let changed = if let Some(bit) = modifier_bit(usage_u16) {
                    let prev = self.physical_modifiers;
                    if pressed {
                        self.physical_modifiers |= bit;
                    } else {
                        self.physical_modifiers &= !bit;
                    }
                    let new_eff = self.effective_modifiers();
                    let diff = new_eff != self.keyboard.modifiers;
                    self.keyboard.modifiers = new_eff;
                    diff || prev != self.physical_modifiers && diff
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

    #[test]
    fn quick_tap_emits_escape() {
        let mut r = Remapper::new(Duration::from_millis(500));
        let t0 = Instant::now();

        let down = r.handle_event(PAGE_KEYBOARD, KEY_CAPS_LOCK as u32, true, t0);
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
        let mut r = Remapper::new(Duration::from_millis(500));
        let t0 = Instant::now();

        r.handle_event(PAGE_KEYBOARD, KEY_CAPS_LOCK as u32, true, t0);
        let up = r.handle_event(
            PAGE_KEYBOARD,
            KEY_CAPS_LOCK as u32,
            false,
            t0 + Duration::from_millis(600),
        );
        assert_eq!(up.len(), 1);
        assert!(matches!(up[0], Action::Send(HidReport::Keyboard(_))));
    }

    #[test]
    fn chord_with_other_key_is_ctrl_only() {
        let mut r = Remapper::new(Duration::from_millis(500));
        let t0 = Instant::now();

        r.handle_event(PAGE_KEYBOARD, KEY_CAPS_LOCK as u32, true, t0);

        let c_down = r.handle_event(
            PAGE_KEYBOARD,
            KEY_C,
            true,
            t0 + Duration::from_millis(20),
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
        );

        let caps_up = r.handle_event(
            PAGE_KEYBOARD,
            KEY_CAPS_LOCK as u32,
            false,
            t0 + Duration::from_millis(80),
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
        let mut r = Remapper::new(Duration::from_millis(500));
        let t0 = Instant::now();

        // Hold physical Left Control, then press & release Caps Lock.
        r.handle_event(PAGE_KEYBOARD, KEY_LEFT_CTRL as u32, true, t0);
        let caps_down = r.handle_event(
            PAGE_KEYBOARD,
            KEY_CAPS_LOCK as u32,
            true,
            t0 + Duration::from_millis(10),
        );
        // Left Control is already set, so no redundant report needed.
        assert!(caps_down.is_empty());

        let caps_up = r.handle_event(
            PAGE_KEYBOARD,
            KEY_CAPS_LOCK as u32,
            false,
            t0 + Duration::from_millis(40),
        );
        // Physical Left Control is still held!
        assert_eq!(r.effective_modifiers(), MOD_LEFT_CTRL);
        assert!(matches!(caps_up.last(), Some(Action::TapEscape(_))));
    }
}
