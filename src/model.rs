//! Gerätestatus, Warnregeln, eigene Namen und Kurzbeschriftungen
//! (Port von devices.h, alert_rules.h, device_aliases.h und labels.cpp aus BatteryHub).

use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub enum DeviceKind {
    #[default]
    Other,
    Mouse,
    Headset,
    Keyboard,
    Gamepad,
    Phone,
    Microphone,
}

/// Ein akkubetriebenes Gerät, wie es bei einer Abfrage gesehen wurde.
#[derive(Clone, Debug)]
pub struct DeviceStatus {
    pub key: String,
    pub name: String,
    pub via: String,
    pub kind: DeviceKind,
    pub percent: i32,
    pub detail: String,
    pub level_text: String,
    pub charging: bool,
    pub full: bool,
    pub asleep: bool,
    pub connected: bool,
    pub grace_sec: i32,
    pub vid: u16,
}

impl Default for DeviceStatus {
    fn default() -> Self {
        DeviceStatus {
            key: String::new(),
            name: String::new(),
            via: String::new(),
            kind: DeviceKind::Other,
            percent: -1,
            detail: String::new(),
            level_text: String::new(),
            charging: false,
            full: false,
            asleep: false,
            connected: true,
            grace_sec: 0,
            vid: 0,
        }
    }
}

// ---------- Warnregeln (alert_rules.h) ----------

pub mod alerts {
    use super::DeviceStatus;
    use std::collections::BTreeMap;

    pub const DEFAULT_THRESHOLD: i32 = 15;
    pub const USE_DEFAULT: i32 = -2;
    pub const DISABLED: i32 = -1;
    pub const THRESHOLDS: [i32; 8] = [5, 10, 15, 20, 25, 30, 35, 40];

    pub fn valid_override(v: i32) -> bool {
        v == DISABLED || THRESHOLDS.contains(&v)
    }

    pub fn override_for(o: &BTreeMap<String, i32>, key: &str) -> i32 {
        o.get(key).copied().unwrap_or(USE_DEFAULT)
    }

    pub fn effective_threshold(o: &BTreeMap<String, i32>, key: &str) -> i32 {
        let v = override_for(o, key);
        if v == USE_DEFAULT { DEFAULT_THRESHOLD } else { v }
    }

    /// Nur für frische Messwerte aufrufen.
    pub fn should_notify(s: &DeviceStatus, threshold: i32, warned: &mut bool) -> bool {
        if threshold == DISABLED {
            *warned = false;
            return false;
        }
        if !s.connected || s.asleep || s.percent < 0 || s.percent > 100 {
            return false;
        }
        if s.charging {
            *warned = false;
            return false;
        }
        if s.percent > threshold + 10 {
            *warned = false;
            return false;
        }
        if s.percent <= threshold && !*warned {
            *warned = true;
            return true;
        }
        false
    }
}

// ---------- Eigene Gerätenamen (device_aliases.h) ----------

pub mod aliases {
    pub const MAX_NAME_LENGTH: usize = 64;
    pub const MAX_KEY_LENGTH: usize = 512;
    pub const MAX_ALIASES: usize = 128;

    pub fn trim(name: &str) -> String {
        name.trim_matches(|c| c == ' ' || c == '\t' || c == '\r' || c == '\n').to_string()
    }

    pub fn valid_name(name: &str) -> bool {
        let n = name.encode_utf16().count();
        n > 0 && n <= MAX_NAME_LENGTH && !name.chars().any(|c| (c as u32) < 32 || c as u32 == 127)
    }

    /// REG_MULTI_SZ: key, alias, key, alias, ...
    pub fn encode(names: &std::collections::BTreeMap<String, String>) -> Vec<String> {
        let mut out = Vec::new();
        for (k, v) in names.iter().take(MAX_ALIASES) {
            if k.is_empty() || k.len() > MAX_KEY_LENGTH || !valid_name(v) {
                continue;
            }
            out.push(k.clone());
            out.push(v.clone());
        }
        out
    }

    pub fn decode(items: &[String]) -> std::collections::BTreeMap<String, String> {
        let mut m = std::collections::BTreeMap::new();
        for pair in items.chunks(2) {
            if m.len() >= MAX_ALIASES {
                break;
            }
            if let [k, v] = pair {
                if !k.is_empty() && k.len() <= MAX_KEY_LENGTH && valid_name(v) {
                    m.insert(k.clone(), v.clone());
                }
            }
        }
        m
    }
}

// ---------- Kurzbeschriftungen (labels.cpp) ----------

const SKIP: &[&str] = &[
    "logitech", "logi", "razer", "steelseries", "corsair", "hyperx", "glorious", "keychron", "microsoft", "sony", "asus",
    "lamzu", "pulsar", "zowie", "rapoo", "wireless", "gaming", "mouse", "headset", "headphones", "keyboard", "controller",
    "gamepad", "receiver", "bluetooth", "usb",
];

fn words(name: &str) -> Vec<String> {
    let all: Vec<String> = name.split_whitespace().map(|s| s.to_string()).collect();
    let kept: Vec<String> = all.iter().filter(|w| !SKIP.contains(&w.to_lowercase().as_str())).cloned().collect();
    if kept.is_empty() { all } else { kept }
}

fn clip(s: &str, max: usize) -> String {
    let c: String = s.chars().take(max).collect();
    c.trim_end().to_string()
}

fn all_distinct(v: &[String]) -> bool {
    let mut seen = BTreeSet::new();
    v.iter().all(|s| !s.is_empty() && seen.insert(s.clone()))
}

/// Bei zwei Geräten derselben Art: eine kurze Beschriftung je Gerät (höchstens `max` Zeichen).
pub fn short_labels(items: &[(DeviceKind, String)], max: usize) -> Vec<String> {
    let mut out = vec![String::new(); items.len()];
    let kinds: BTreeSet<DeviceKind> = items.iter().map(|i| i.0).collect();
    for kind in kinds {
        let idx: Vec<usize> = (0..items.len()).filter(|&i| items[i].0 == kind).collect();
        if idx.len() < 2 {
            continue;
        }
        let ws: Vec<Vec<String>> = idx.iter().map(|&i| words(&items[i].1)).collect();
        let most = ws.iter().map(|w| w.len()).max().unwrap_or(1).max(1);
        let mut pick: Vec<String> = Vec::new();
        for k in 1..=most {
            let cand: Vec<String> = ws.iter().map(|w| clip(&w.iter().take(k).cloned().collect::<Vec<_>>().join(" "), max)).collect();
            if all_distinct(&cand) {
                pick = cand;
                break;
            }
        }
        if pick.is_empty() {
            let nums: Vec<String> = ws
                .iter()
                .map(|w| match w.last() {
                    Some(l) if !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()) => clip(l, max),
                    _ => String::new(),
                })
                .collect();
            pick = if all_distinct(&nums) { nums } else { (1..=idx.len()).map(|n| n.to_string()).collect() };
        }
        for (n, &i) in idx.iter().enumerate() {
            out[i] = pick[n].clone();
        }
    }
    out
}

pub type AliasMap = BTreeMap<String, String>;
