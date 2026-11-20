//! USB HID usage constants and packed report layouts matching
//! `pqrs::karabiner::driverkit::virtual_hid_device_driver::hid_report`.

pub const PAGE_GENERIC_DESKTOP: u32 = 0x01;
pub const PAGE_KEYBOARD: u32 = 0x07;
pub const PAGE_CONSUMER: u32 = 0x0C;
pub const PAGE_APPLE_VENDOR_TOP_CASE: u32 = 0x00FF;
pub const PAGE_APPLE_VENDOR_KEYBOARD: u32 = 0xFF01;

pub const KEY_ESCAPE: u16 = 0x29;
pub const KEY_CAPS_LOCK: u16 = 0x39;
pub const KEY_LEFT_CTRL: u16 = 0xE0;
pub const KEY_RIGHT_GUI: u16 = 0xE7;

/// AppleVendorTopCase usages for the display brightness keys.
pub const APPLE_TOP_CASE_BRIGHTNESS_UP: u16 = 0x0004;
pub const APPLE_TOP_CASE_BRIGHTNESS_DOWN: u16 = 0x0005;

/// Consumer-page brightness usages, which macOS accepts from a virtual keyboard.
/// Karabiner maps the top row to these rather than to the AppleVendorTopCase ones.
pub const CONSUMER_DISPLAY_BRIGHTNESS_INCREMENT: u16 = 0x006F;
pub const CONSUMER_DISPLAY_BRIGHTNESS_DECREMENT: u16 = 0x0070;

pub const MOD_LEFT_CTRL: u8 = 1 << 0;

/// Map a Keyboard/Keypad usage (0xE0..=0xE7) to its USB HID modifier bit.
#[inline]
pub fn modifier_bit(usage: u16) -> Option<u8> {
    if (KEY_LEFT_CTRL..=KEY_RIGHT_GUI).contains(&usage) {
        Some(1u8 << (usage - KEY_LEFT_CTRL))
    } else {
        None
    }
}

/// 32-slot key array matching `hid_report::keys` (`uint16_t keys_[32]`, packed LE).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeySlots {
    keys: [u16; 32],
}

impl Default for KeySlots {
    fn default() -> Self {
        Self { keys: [0; 32] }
    }
}

impl KeySlots {
    pub fn insert(&mut self, key: u16) -> bool {
        if key == 0 || self.contains(key) {
            return false;
        }
        for slot in &mut self.keys {
            if *slot == 0 {
                *slot = key;
                return true;
            }
        }
        false
    }

    pub fn erase(&mut self, key: u16) -> bool {
        let mut changed = false;
        for slot in &mut self.keys {
            if *slot == key {
                *slot = 0;
                changed = true;
            }
        }
        changed
    }

    pub fn contains(&self, key: u16) -> bool {
        self.keys.contains(&key)
    }

    pub fn clear(&mut self) {
        self.keys.fill(0);
    }

    pub fn write_le_bytes(&self, out: &mut Vec<u8>) {
        for &k in &self.keys {
            out.extend_from_slice(&k.to_le_bytes());
        }
    }
}

/// Packed `hid_report::keyboard_input` (67 bytes):
/// `[report_id=1, modifiers, reserved=0, keys[32] LE]`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct KeyboardReport {
    pub modifiers: u8,
    pub keys: KeySlots,
}

impl KeyboardReport {
    pub const REPORT_ID: u8 = 1;
    pub const BYTE_LEN: usize = 1 + 1 + 1 + 32 * 2;

    pub fn to_bytes(&self) -> [u8; Self::BYTE_LEN] {
        let mut buf = Vec::with_capacity(Self::BYTE_LEN);
        buf.push(Self::REPORT_ID);
        buf.push(self.modifiers);
        buf.push(0); // reserved
        self.keys.write_le_bytes(&mut buf);
        buf.try_into().expect("keyboard report is 67 bytes")
    }
}

/// Packed 65-byte report for Consumer (2), AppleVendorTopCase (3),
/// AppleVendorKeyboard (4), and GenericDesktop (7).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct AuxiliaryReport {
    pub keys: KeySlots,
}

impl AuxiliaryReport {
    pub const BYTE_LEN: usize = 1 + 32 * 2;

    pub fn to_bytes(&self, report_id: u8) -> [u8; Self::BYTE_LEN] {
        let mut buf = Vec::with_capacity(Self::BYTE_LEN);
        buf.push(report_id);
        self.keys.write_le_bytes(&mut buf);
        buf.try_into().expect("auxiliary report is 65 bytes")
    }
}

/// Report categories supported by `Karabiner-VirtualHIDDevice-Daemon`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HidReport {
    Keyboard(KeyboardReport),
    Consumer(AuxiliaryReport),
    AppleVendorTopCase(AuxiliaryReport),
    AppleVendorKeyboard(AuxiliaryReport),
    GenericDesktop(AuxiliaryReport),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_report_wire_layout() {
        let mut rep = KeyboardReport::default();
        rep.modifiers = MOD_LEFT_CTRL;
        rep.keys.insert(KEY_ESCAPE);
        let bytes = rep.to_bytes();
        assert_eq!(bytes.len(), 67);
        assert_eq!(bytes[0], 1); // report_id
        assert_eq!(bytes[1], 0x01); // left_control
        assert_eq!(bytes[2], 0x00); // reserved
        assert_eq!(&bytes[3..5], &KEY_ESCAPE.to_le_bytes());
        assert!(bytes[5..].iter().all(|&b| b == 0));
    }

    #[test]
    fn auxiliary_report_wire_layout() {
        let mut rep = AuxiliaryReport::default();
        rep.keys.insert(0x0003); // Fn/Globe
        let bytes = rep.to_bytes(3);
        assert_eq!(bytes.len(), 65);
        assert_eq!(bytes[0], 3);
        assert_eq!(&bytes[1..3], &0x0003u16.to_le_bytes());
    }
}
