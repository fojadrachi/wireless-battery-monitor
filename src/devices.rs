//! Akku-Abfrage der unterstützten Geräte über HID.
//!
//! - ASTRO A50 Gen 5 (046D:0B1C, FF32/0074): `02 0c 03 00 06 0c`, Antwort byte6 = %, byte8 = lädt
//!   (wie BatteryHub / HeadsetControl)
//! - ASTRO A50 Gen 4 (9886:002C, FF32): Report-ID 2; `54` = Dock/Verbindung (bit0 Dock, bit1 Headset an),
//!   `7C` = Akku (bit7 lädt, bit0-6 %) (wie BatteryHub / eh-fifty)
//! - AJAZZ 2.4-GHz-Empfänger (249A:5C2F / 248A:5C2F, Interface 2): GET `10` (Geräte-Info),
//!   Antwort byte13 = % (wie Ajazzy)

use crate::model::{DeviceKind, DeviceStatus};
use hidapi::{DeviceInfo, HidApi, HidDevice};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static CACHE: Mutex<Option<HashMap<u16, (i32, bool, bool)>>> = Mutex::new(None);

fn read_until(h: &HidDevice, ms: u64, mut accept: impl FnMut(&[u8]) -> bool) -> Option<Vec<u8>> {
    let end = Instant::now() + Duration::from_millis(ms);
    for _ in 0..12 {
        let left = end.saturating_duration_since(Instant::now()).as_millis() as i32;
        if left <= 0 {
            break;
        }
        let mut buf = [0u8; 65];
        match h.read_timeout(&mut buf, left) {
            Ok(0) => return None,
            Ok(n) if accept(&buf[..n]) => return Some(buf[..n].to_vec()),
            Ok(_) => continue,
            Err(_) => return None,
        }
    }
    None
}

fn flush(h: &HidDevice) {
    let mut buf = [0u8; 65];
    for _ in 0..32 {
        match h.read_timeout(&mut buf, 0) {
            Ok(n) if n > 0 => continue,
            _ => break,
        }
    }
}

fn base(key: String, name: &str, kind: DeviceKind, via: &str, vid: u16) -> DeviceStatus {
    DeviceStatus { key, name: name.into(), kind, via: via.into(), vid, ..Default::default() }
}

// ---------- ASTRO A50 Gen 5 ----------

fn poll_a50_gen5(api: &HidApi, d: &DeviceInfo) -> Option<DeviceStatus> {
    let h = d.open_device(api).ok()?;
    flush(&h);
    let mut req = [0u8; 64];
    req[..6].copy_from_slice(&[0x02, 0x0C, 0x03, 0x00, 0x06, 0x0C]);
    h.write(&req).ok()?;
    let r = read_until(&h, 1000, |r| {
        r.len() >= 9 && r[0] == 0x02 && r[1] == 0x0C && r[2] >= 6 && (r[2] as usize + 3) <= r.len() && r[3] == 0
            && r[4] == 0x06 && (r[5] == 0 || r[5] == 0x0C) && r[6] <= 100
    })?;
    let mut s = base(format!("astro:{:04X}", d.product_id()), "ASTRO A50 Gen 5", DeviceKind::Headset, "USB base station", 0x046D);
    s.percent = r[6] as i32;
    s.charging = r[8] != 0;
    Some(s)
}

// ---------- ASTRO A50 Gen 4 ----------

enum Q {
    Ok(u8),
    Error,
    NoReply,
}

fn query_legacy(h: &HidDevice, cmd: u8) -> Q {
    flush(h);
    let mut req = [0u8; 64];
    req[0] = 0x02;
    req[1] = cmd;
    if h.write(&req).is_err() {
        return Q::NoReply;
    }
    match read_until(h, 1500, |r| r.len() >= 3 && r[0] == 0x02 && r[1] <= 2) {
        Some(r) if r[1] == 2 && r[2] >= 1 && r.len() >= 4 => Q::Ok(r[3]),
        Some(r) if r[1] == 1 => Q::Error,
        _ => Q::NoReply,
    }
}

fn poll_a50_gen4(api: &HidApi, d: &DeviceInfo) -> Option<DeviceStatus> {
    let pid = d.product_id();
    let mut s = base(format!("astro:{pid:04X}"), "ASTRO A50 Gen 4", DeviceKind::Headset, "USB base station", 0x9886);
    s.grace_sec = 1800;
    let Ok(h) = d.open_device(api) else {
        // Exklusiv gesperrt (z. B. Astro Command Center): letzten Wert behalten.
        s.connected = true;
        if let Some((pct, chg, docked)) = CACHE.lock().ok()?.as_ref().and_then(|c| c.get(&pid).copied()) {
            s.percent = pct;
            s.charging = chg;
            s.detail = if docked { "docked, Astro Command Center".into() } else { "Astro Command Center".into() };
        } else {
            s.detail = "Astro Command Center".into();
        }
        return Some(s);
    };
    let status = match query_legacy(&h, 0x54) {
        Q::Ok(v) => Some(v),
        _ => None,
    };
    let docked = status.map(|st| st & 0x01 != 0).unwrap_or(false);
    if let Some(st) = status {
        s.connected = docked || st & 0x02 != 0;
        if !s.connected {
            return Some(s);
        }
    }
    match query_legacy(&h, 0x7C) {
        Q::Ok(bat) if (bat & 0x7F) <= 100 => {
            let pct = (bat & 0x7F) as i32;
            s.connected = true;
            s.percent = pct;
            s.charging = bat & 0x80 != 0 || (docked && pct < 100);
            if docked {
                s.detail = "docked".into();
            }
            if let Ok(mut c) = CACHE.lock() {
                c.get_or_insert_with(HashMap::new).insert(pid, (pct, s.charging, docked));
            }
            Some(s)
        }
        Q::Error if status.is_none() => {
            s.connected = false;
            Some(s)
        }
        _ => status.map(|_| s),
    }
}

// ---------- AJAZZ ----------

fn poll_ajazz(api: &HidApi, d: &DeviceInfo) -> Option<DeviceStatus> {
    let h = d.open_device(api).ok()?;
    flush(&h);
    let mut req = [0u8; 33];
    req[1] = 0x10; // GET Geräte-Info, Prüfsumme 0
    h.write(&req).ok()?;
    let r = read_until(&h, 1200, |r| r.len() >= 14 && r[0] == 0x10 && r[2] == 0x01);
    let model = r
        .as_ref()
        .map(|r| String::from_utf8_lossy(&r[4..8]).trim_matches(char::from(0)).trim().to_string())
        .filter(|m| !m.is_empty() && m.chars().all(|c| c.is_ascii_alphanumeric()));
    let name = match &model {
        Some(m) => format!("AJAZZ AJ{m}"),
        None => "AJAZZ Wireless Mouse".into(),
    };
    let mut s = base(format!("ajazz:{:04X}:{:04X}", d.vendor_id(), d.product_id()), &name, DeviceKind::Mouse, "2.4 GHz receiver", d.vendor_id());
    s.grace_sec = 600;
    match r {
        Some(r) if (1..=100).contains(&r[13]) => s.percent = r[13] as i32,
        _ => s.asleep = true,
    }
    Some(s)
}

/// Fragt alle unterstützten Geräte ab. Blockiert (bis ~2 s); aus dem Arbeits-Thread aufrufen.
pub fn poll_all(api: &mut HidApi) -> Vec<DeviceStatus> {
    let _ = api.refresh_devices();
    let list: Vec<DeviceInfo> = api.device_list().cloned().collect();
    let mut out: Vec<DeviceStatus> = Vec::new();
    let mut add = |s: Option<DeviceStatus>| {
        if let Some(s) = s {
            if !out.iter().any(|o| o.key == s.key) {
                out.push(s);
            }
        }
    };
    for d in &list {
        let (v, p) = (d.vendor_id(), d.product_id());
        if v == 0x046D && p == 0x0B1C && d.usage_page() == 0xFF32 && d.usage() == 0x0074 {
            add(poll_a50_gen5(api, d));
        } else if v == 0x9886 && matches!(p, 0x002C | 0x002D | 0x002A) && matches!(d.usage_page(), 0xFF32 | 0xFF31 | 0xFF00) {
            add(poll_a50_gen4(api, d));
        } else if matches!(v, 0x249A | 0x248A) && p == 0x5C2F && d.interface_number() == 2 {
            add(poll_ajazz(api, d));
        }
    }
    // Wie im Original: Bluetooth-Geräte mit Windows-Akkustand anhängen.
    let bt = crate::bluetooth::poll(&out);
    out.extend(bt);
    out
}
