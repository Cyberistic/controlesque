//! Per-keyboard `fn` usage tables published by Apple's keyboard driver.
//!
//! `AppleHIDKeyboardEventDriverV2` describes, for each keyboard it drives:
//!
//! - `FnFunctionUsageMap`: what each top-row key (F1..F12) becomes when the
//!   keyboard is *not* in "standard function keys" mode.
//! - `FnKeyboardUsageMap`: what `fn` + a key becomes (arrows, forward delete).
//! - `FnModifierUsagePage` / `FnModifierUsage`: which usage is the `fn` key.
//!
//! Seizing a device bypasses that driver, which is why the top row would
//! otherwise arrive as plain F-keys. Replaying the driver's own tables keeps
//! the behaviour correct for whatever Apple keyboard (and macOS version) is in
//! use, including keyboards that publish no table at all — those keep passing
//! their keys through untouched.

/// Usage encoded as `(page << 16) | usage`, the format used by the tables.
pub type PackedUsage = u32;

/// Fallback `fn` usage (`AppleVendorTopCase` / `keyboard_fn`) for keyboards
/// that do not publish their own.
pub const DEFAULT_FN_PAGE: u16 = 0x00FF;
pub const DEFAULT_FN_USAGE: u16 = 0x0003;

/// Pack a `(usage page, usage)` pair into the tables' 32-bit encoding.
pub fn pack(page: u32, usage: u32) -> PackedUsage {
    ((page & 0xFFFF) << 16) | (usage & 0xFFFF)
}

/// Parse a `0x0007003a,0x00ff0005,...` property into `(from, to)` pairs.
///
/// Unparsable values are skipped, as are trailing unpaired values.
pub fn parse_pairs(text: &str) -> Vec<(PackedUsage, PackedUsage)> {
    let mut values = text.split(',').filter_map(|token| {
        let token = token.trim();
        let digits = token
            .strip_prefix("0x")
            .or_else(|| token.strip_prefix("0X"))
            .unwrap_or(token);
        u32::from_str_radix(digits, 16).ok()
    });

    let mut pairs = Vec::new();
    while let (Some(from), Some(to)) = (values.next(), values.next()) {
        pairs.push((from, to));
    }
    pairs
}

/// Which usage is `fn`, and what the top row / `fn` chords become.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnUsageMap {
    functions: Vec<(PackedUsage, PackedUsage)>,
    keyboard: Vec<(PackedUsage, PackedUsage)>,
    fn_page: u16,
    fn_usage: u16,
}

impl Default for FnUsageMap {
    fn default() -> Self {
        Self {
            functions: Vec::new(),
            keyboard: Vec::new(),
            fn_page: DEFAULT_FN_PAGE,
            fn_usage: DEFAULT_FN_USAGE,
        }
    }
}

impl FnUsageMap {
    pub fn from_parts(
        functions: Option<&str>,
        keyboard: Option<&str>,
        fn_page: Option<u16>,
        fn_usage: Option<u16>,
    ) -> Self {
        let mut map = Self::default();
        if let Some(text) = functions {
            map.functions = parse_pairs(text);
        }
        if let Some(text) = keyboard {
            map.keyboard = parse_pairs(text);
        }
        if let Some(page) = fn_page {
            map.fn_page = page;
        }
        if let Some(usage) = fn_usage {
            map.fn_usage = usage;
        }
        map
    }

    /// Top-row target, i.e. what F1..F12 become outside standard function mode.
    pub fn function(&self, from: PackedUsage) -> Option<PackedUsage> {
        lookup(&self.functions, from)
    }

    /// Target of `fn` + key (Home/End/Page Up/Page Down/Forward Delete).
    pub fn keyboard(&self, from: PackedUsage) -> Option<PackedUsage> {
        lookup(&self.keyboard, from)
    }

    pub fn fn_page(&self) -> u16 {
        self.fn_page
    }

    pub fn fn_usage(&self) -> u16 {
        self.fn_usage
    }

    /// True when the keyboard publishes neither table, i.e. its keys should be
    /// forwarded as-is.
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty() && self.keyboard.is_empty()
    }
}

fn lookup(pairs: &[(PackedUsage, PackedUsage)], from: PackedUsage) -> Option<PackedUsage> {
    pairs
        .iter()
        .find(|(source, _)| *source == from)
        .map(|(_, target)| *target)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FUNCTIONS: &str = "0x0007003a,0x00ff0005,0x0007003b,0x00ff0004,0x0007003c,0xff010010,0x0007003d,0x000c0221,0x0007003e,0x000c00cf,0x0007003f,0x0001009b,0x00070040,0x000c00b4,0x00070041,0x000c00cd,0x00070042,0x000c00b3,0x00070043,0x000c00e2,0x00070044,0x000c00ea,0x00070045,0x000c00e9";
    const KEYBOARD: &str = "0x00070050,0x0007004a,0x00070052,0x0007004b,0x0007002a,0x0007004c,0x0007004f,0x0007004d,0x00070051,0x0007004e,0x00070028,0x00070058";

    #[test]
    fn parses_apple_tables() {
        let map = FnUsageMap::from_parts(Some(FUNCTIONS), Some(KEYBOARD), Some(0xFF), Some(3));

        assert_eq!(map.function(pack(0x07, 0x3A)), Some(pack(0x00FF, 0x05)));
        assert_eq!(map.function(pack(0x07, 0x3B)), Some(pack(0x00FF, 0x04)));
        assert_eq!(map.function(pack(0x07, 0x3C)), Some(pack(0xFF01, 0x10)));
        assert_eq!(map.function(pack(0x07, 0x3D)), Some(pack(0x0C, 0x0221)));
        assert_eq!(map.function(pack(0x07, 0x45)), Some(pack(0x0C, 0x00E9)));
        assert_eq!(map.function(pack(0x07, 0x04)), None);

        assert_eq!(map.keyboard(pack(0x07, 0x50)), Some(pack(0x07, 0x4A)));
        assert_eq!(map.keyboard(pack(0x07, 0x2A)), Some(pack(0x07, 0x4C)));
        assert_eq!(map.keyboard(pack(0x07, 0x3B)), None);

        assert_eq!(map.fn_page(), 0x00FF);
        assert_eq!(map.fn_usage(), 0x0003);
        assert!(!map.is_empty());
    }

    #[test]
    fn empty_map_keeps_keys_untouched() {
        let map = FnUsageMap::default();
        assert!(map.is_empty());
        assert_eq!(map.function(pack(0x07, 0x3B)), None);
        assert_eq!(map.keyboard(pack(0x07, 0x50)), None);
        assert_eq!(map.fn_page(), 0x00FF);
        assert_eq!(map.fn_usage(), 0x0003);
    }

    #[test]
    fn ignores_malformed_entries() {
        let pairs = parse_pairs("0x0007003a, nope, 0x00ff0005, 0x0007003b");
        assert_eq!(pairs, vec![(pack(0x07, 0x3A), pack(0x00FF, 0x05))]);
    }
}
