//! Markenfarben (Port von brands.cpp aus BatteryHub).

use crate::model::{DeviceKind, DeviceStatus};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Accent {
    #[default]
    None,
    Razer, Logitech, SteelSeries, Corsair, HyperX, Rog, Zowie,
    PlayStation, Xbox, Galaxy, Apple, Jabra, Jbl, CoolerMaster, Steam, Redragon,
}

pub struct BrandStyle {
    pub accent: Accent,
    pub name: &'static str,
    pub dark: (u8, u8, u8),
    pub light: (u8, u8, u8),
}

const BRANDS: &[BrandStyle] = &[
    BrandStyle { accent: Accent::None, name: "", dark: (255, 255, 255), light: (0, 0, 0) },
    BrandStyle { accent: Accent::Razer, name: "Razer", dark: (68, 214, 44), light: (22, 140, 20) },
    BrandStyle { accent: Accent::Logitech, name: "Logitech", dark: (0, 184, 252), light: (0, 120, 180) },
    BrandStyle { accent: Accent::SteelSeries, name: "SteelSeries", dark: (255, 82, 0), light: (204, 65, 0) },
    BrandStyle { accent: Accent::Corsair, name: "Corsair", dark: (245, 228, 26), light: (150, 122, 0) },
    BrandStyle { accent: Accent::HyperX, name: "HyperX", dark: (232, 35, 63), light: (200, 16, 46) },
    BrandStyle { accent: Accent::Rog, name: "ROG", dark: (255, 46, 77), light: (208, 0, 42) },
    BrandStyle { accent: Accent::Zowie, name: "ZOWIE", dark: (240, 60, 50), light: (192, 32, 26) },
    BrandStyle { accent: Accent::PlayStation, name: "PlayStation", dark: (31, 139, 255), light: (0, 98, 196) },
    BrandStyle { accent: Accent::Xbox, name: "Xbox", dark: (34, 168, 34), light: (16, 124, 16) },
    BrandStyle { accent: Accent::Galaxy, name: "Samsung", dark: (91, 123, 255), light: (20, 40, 160) },
    BrandStyle { accent: Accent::Apple, name: "Apple", dark: (199, 199, 204), light: (110, 110, 115) },
    BrandStyle { accent: Accent::Jabra, name: "Jabra", dark: (255, 200, 0), light: (140, 106, 0) },
    BrandStyle { accent: Accent::Jbl, name: "JBL", dark: (255, 61, 0), light: (204, 52, 0) },
    BrandStyle { accent: Accent::CoolerMaster, name: "Cooler Master", dark: (157, 107, 255), light: (106, 43, 217) },
    BrandStyle { accent: Accent::Steam, name: "Steam", dark: (26, 159, 255), light: (11, 111, 191) },
    BrandStyle { accent: Accent::Redragon, name: "Redragon", dark: (230, 0, 18), light: (192, 0, 15) },
];

pub fn style_of(a: Accent) -> &'static BrandStyle {
    BRANDS.iter().find(|b| b.accent == a).unwrap_or(&BRANDS[0])
}

pub fn accent_color(a: Accent, light: bool) -> (u8, u8, u8) {
    let b = style_of(a);
    if light { b.light } else { b.dark }
}

fn has_word(n: &str, w: &str) -> bool {
    let nb: Vec<char> = n.chars().collect();
    let wl = w.chars().count();
    let mut i = 0;
    while i + wl <= nb.len() {
        if nb[i..i + wl].iter().collect::<String>() == w {
            let start_ok = i == 0 || !nb[i - 1].is_alphanumeric();
            let end_ok = i + wl == nb.len() || !nb[i + wl].is_alphanumeric();
            if start_ok && end_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

const KEYS: &[(&str, Accent)] = &[
    ("razer:", Accent::Razer), ("blackshark:", Accent::Razer), ("barracuda:", Accent::Razer),
    ("logi:", Accent::Logitech), ("g435:", Accent::Logitech), ("steelseries:", Accent::SteelSeries),
    ("corsair:", Accent::Corsair), ("hyperx:", Accent::HyperX), ("rog:", Accent::Rog), ("zowie:", Accent::Zowie),
    ("ps:", Accent::PlayStation), ("inzone:", Accent::PlayStation), ("xbox:", Accent::Xbox), ("xinput:", Accent::Xbox),
    ("galaxybuds:", Accent::Galaxy), ("airpods:", Accent::Apple), ("jabra:", Accent::Jabra), ("jbl:", Accent::Jbl),
    ("coolermaster:", Accent::CoolerMaster), ("steam:", Accent::Steam), ("redragon:", Accent::Redragon),
];

const VIDS: &[(u16, Accent)] = &[
    (0x1532, Accent::Razer), (0x068E, Accent::Razer), (0x046D, Accent::Logitech), (0x1038, Accent::SteelSeries),
    (0x1B1C, Accent::Corsair), (0x0951, Accent::HyperX), (0x04A5, Accent::Zowie), (0x054C, Accent::PlayStation),
    (0x04E8, Accent::Galaxy), (0x05AC, Accent::Apple), (0x004C, Accent::Apple), (0x0B0E, Accent::Jabra),
    (0x0ECB, Accent::Jbl), (0x2516, Accent::CoolerMaster), (0x28DE, Accent::Steam),
];

#[derive(Clone, Copy)]
enum How { Contains, Word, Prefix, Exact }

const NAMES: &[(&str, How, Accent)] = &[
    ("razer", How::Contains, Accent::Razer), ("logitech", How::Contains, Accent::Logitech), ("logi ", How::Prefix, Accent::Logitech),
    ("mx ", How::Prefix, Accent::Logitech), ("lift", How::Prefix, Accent::Logitech), ("pop mouse", How::Prefix, Accent::Logitech),
    ("pop keys", How::Prefix, Accent::Logitech), ("pebble", How::Prefix, Accent::Logitech), ("ergo k", How::Prefix, Accent::Logitech),
    ("ergo m", How::Prefix, Accent::Logitech), ("steelseries", How::Contains, Accent::SteelSeries), ("arctis", How::Word, Accent::SteelSeries),
    ("aerox", How::Word, Accent::SteelSeries), ("rival ", How::Prefix, Accent::SteelSeries), ("corsair", How::Contains, Accent::Corsair),
    ("hyperx", How::Contains, Accent::HyperX), ("pulsefire", How::Word, Accent::HyperX), ("rog", How::Word, Accent::Rog),
    ("zowie", How::Contains, Accent::Zowie), ("dualsense", How::Contains, Accent::PlayStation), ("dualshock", How::Contains, Accent::PlayStation),
    ("playstation", How::Contains, Accent::PlayStation), ("inzone", How::Word, Accent::PlayStation),
    ("wireless controller", How::Exact, Accent::PlayStation), ("xbox", How::Word, Accent::Xbox), ("galaxy ", How::Prefix, Accent::Galaxy),
    ("samsung", How::Word, Accent::Galaxy), ("airpods", How::Contains, Accent::Apple), ("magic mouse", How::Prefix, Accent::Apple),
    ("magic keyboard", How::Prefix, Accent::Apple), ("magic trackpad", How::Prefix, Accent::Apple), ("apple", How::Word, Accent::Apple),
    ("iphone", How::Prefix, Accent::Apple), ("jabra", How::Word, Accent::Jabra), ("jbl", How::Word, Accent::Jbl),
    ("cooler master", How::Contains, Accent::CoolerMaster), ("coolermaster", How::Contains, Accent::CoolerMaster),
    ("steam controller", How::Contains, Accent::Steam), ("redragon", How::Contains, Accent::Redragon),
];

fn matches(n: &str, text: &str, how: How) -> bool {
    match how {
        How::Contains => n.contains(text),
        How::Word => has_word(n, text),
        How::Prefix => n.starts_with(text),
        How::Exact => n == text,
    }
}

fn logitech_model(n: &str) -> bool {
    let c: Vec<char> = n.chars().collect();
    c.len() >= 4 && matches!(c[0], 'g' | 'k' | 'm') && c[1].is_ascii_digit() && c[2].is_ascii_digit() && c[3].is_ascii_digit()
        && (c.len() == 4 || !c[4].is_alphanumeric())
}

fn sony_audio(n: &str) -> bool {
    n.starts_with("wh-") || n.starts_with("wf-") || has_word(n, "linkbuds")
}

/// Marke aus Treiberschlüssel, dann Hersteller-ID, dann Name.
pub fn brand_of(s: &DeviceStatus) -> Accent {
    for (p, a) in KEYS {
        if s.key.starts_with(p) {
            return *a;
        }
    }
    let n = s.name.to_lowercase();
    if s.vid != 0 {
        for (v, a) in VIDS {
            if *v == s.vid && !(*a == Accent::PlayStation && sony_audio(&n)) {
                return *a;
            }
        }
    }
    if s.vid == 0x045E && s.kind == DeviceKind::Gamepad {
        return Accent::Xbox;
    }
    if sony_audio(&n) {
        return Accent::None;
    }
    for (t, h, a) in NAMES {
        if matches(&n, t, *h) {
            return *a;
        }
    }
    if logitech_model(&n) { Accent::Logitech } else { Accent::None }
}
