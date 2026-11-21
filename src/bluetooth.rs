//! Bluetooth-Geräte mit Akkustand von Windows (Port von PollBluetooth aus BatteryHubs generic.cpp).
//!
//! Windows speichert den Akkustand, den es per HFP (klassisch) oder GATT-Battery-Service (LE) liest,
//! in einer Geräteeigenschaft – derselbe Wert, den Einstellungen > Bluetooth & Geräte zeigt.
//! Geräte ohne diesen Wert (z. B. ASTRO A50 per Bluetooth) liefert Windows nicht und werden übersprungen.

use crate::model::{DeviceKind, DeviceStatus};
use std::collections::BTreeMap;
use windows_sys::core::GUID;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Devices::Properties::*;
use windows_sys::Win32::Foundation::DEVPROPKEY;

const BT_BATTERY: DEVPROPKEY = DEVPROPKEY { fmtid: GUID::from_u128(0x104EA319_6EE2_4701_BD47_8DDBF425BBE5), pid: 2 };
const IS_CONNECTED: DEVPROPKEY = DEVPROPKEY { fmtid: GUID::from_u128(0x83DA6326_97A6_4088_9453_A1923F573B29), pid: 15 };
const CLASS_OF_DEVICE: DEVPROPKEY = DEVPROPKEY { fmtid: GUID::from_u128(0x2BD67D8B_8BEB_48D5_87E0_6CDA3428040A), pid: 10 };

fn kind_from_cod(cod: u32) -> DeviceKind {
    let (major, minor) = ((cod >> 8) & 0x1F, (cod >> 2) & 0x3F);
    match major {
        0x02 => DeviceKind::Phone,
        0x04 => DeviceKind::Headset,
        0x05 if (minor & 0x0F) == 0x01 || (minor & 0x0F) == 0x02 => DeviceKind::Gamepad,
        0x05 if (minor >> 4) == 1 => DeviceKind::Keyboard,
        0x05 if (minor >> 4) == 2 => DeviceKind::Mouse,
        _ => DeviceKind::Other,
    }
}

fn kind_from_category(c: &str) -> DeviceKind {
    if c == "Communication.Phone" || c.starts_with("Communication.Phone.") {
        DeviceKind::Phone
    } else if c == "Input.Keyboard" {
        DeviceKind::Keyboard
    } else if c == "Input.Mouse" {
        DeviceKind::Mouse
    } else if c == "Input.Gaming" {
        DeviceKind::Gamepad
    } else if c.starts_with("Communication.Headset") || c == "Audio.Headphones" {
        DeviceKind::Headset
    } else {
        DeviceKind::Other
    }
}

/// Grobe Einordnung über den Namen, wenn Windows nichts sagt.
fn guess_kind(name: &str) -> DeviceKind {
    let n = name.to_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| n.contains(x));
    if has(&["mouse", "maus", "mx master", "mx anywhere", "trackball"]) {
        DeviceKind::Mouse
    } else if has(&["keyboard", "tastatur", "keys"]) {
        DeviceKind::Keyboard
    } else if has(&["controller", "gamepad", "joy-con", "dualsense", "dualshock"]) {
        DeviceKind::Gamepad
    } else if has(&["headset", "headphone", "kopfhörer", "buds", "airpods", "earbuds", "wh-", "wf-", "ear"]) {
        DeviceKind::Headset
    } else {
        DeviceKind::Other
    }
}

fn strip_service_suffix(n: &str) -> String {
    for suf in [" Hands-Free AG", " Hands-Free", " Avrcp Transport", " AVRCP-Transport", " Stereo", " Audio Gateway"] {
        if n.len() > suf.len() && n.ends_with(suf) {
            return n[..n.len() - suf.len()].to_string();
        }
    }
    n.to_string()
}

unsafe fn prop_raw(di: HDEVINFO, dd: &mut SP_DEVINFO_DATA, key: &DEVPROPKEY, buf: &mut [u8]) -> Option<u32> {
    let mut t: DEVPROPTYPE = 0;
    let ok = unsafe { SetupDiGetDevicePropertyW(di, dd, key, &mut t, buf.as_mut_ptr(), buf.len() as u32, std::ptr::null_mut(), 0) };
    (ok != 0).then_some(t)
}

unsafe fn str_prop(di: HDEVINFO, dd: &mut SP_DEVINFO_DATA, key: &DEVPROPKEY, list_ok: bool) -> String {
    let mut buf = [0u8; 512];
    match unsafe { prop_raw(di, dd, key, &mut buf[..510]) } {
        Some(t) if t == DEVPROP_TYPE_STRING || (list_ok && t == DEVPROP_TYPE_STRING_LIST) => {
            let w: Vec<u16> = buf.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            crate::util::from_w(&w)
        }
        _ => String::new(),
    }
}

fn guid_string(g: &GUID) -> String {
    format!("{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}", g.data1, g.data2, g.data3, g.data4[0], g.data4[1],
            g.data4[2], g.data4[3], g.data4[4], g.data4[5], g.data4[6], g.data4[7])
}

struct Found {
    pct: u8,
    name: String,
    connected: bool,
}

struct Scan {
    container_name: BTreeMap<String, String>,
    container_cod: BTreeMap<String, u32>,
    container_cat: BTreeMap<String, String>,
    container_conn: BTreeMap<String, bool>,
    battery: BTreeMap<String, Found>,
}

/// Verbundene Bluetooth-Geräte, für die Windows keinen Akkustand hat (für `--list`).
pub fn without_battery() -> Vec<String> {
    let sc = scan();
    sc.container_name
        .iter()
        .filter(|(cid, _)| sc.container_conn.get(*cid).copied().unwrap_or(false) && !sc.battery.contains_key(*cid))
        .map(|(_, n)| n.clone())
        .collect()
}

fn scan() -> Scan {
    let mut container_name: BTreeMap<String, String> = BTreeMap::new();
    let mut container_cod: BTreeMap<String, u32> = BTreeMap::new();
    let mut container_cat: BTreeMap<String, String> = BTreeMap::new();
    let mut container_conn: BTreeMap<String, bool> = BTreeMap::new();
    let mut battery: BTreeMap<String, Found> = BTreeMap::new();
    unsafe {
        let di = SetupDiGetClassDevsW(std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), DIGCF_ALLCLASSES | DIGCF_PRESENT);
        if di == -1isize as HDEVINFO {
            return Scan { container_name, container_cod, container_cat, container_conn, battery };
        }
        let mut dd: SP_DEVINFO_DATA = std::mem::zeroed();
        dd.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
        let mut i = 0;
        while SetupDiEnumDeviceInfo(di, i, &mut dd) != 0 {
            i += 1;
            let mut inst = [0u16; 512];
            if SetupDiGetDeviceInstanceIdW(di, &dd, inst.as_mut_ptr(), 512, std::ptr::null_mut()) == 0 {
                continue;
            }
            let inst = crate::util::from_w(&inst);
            if !(inst.starts_with("BTHENUM\\") || inst.starts_with("BTHLE\\") || inst.starts_with("BTHLEDEVICE\\")) {
                continue;
            }
            let mut cid = inst.clone();
            let mut gbuf = [0u8; 16];
            if prop_raw(di, &mut dd, &DEVPKEY_Device_ContainerId, &mut gbuf).is_some() {
                let g: GUID = std::ptr::read_unaligned(gbuf.as_ptr() as *const GUID);
                cid = guid_string(&g);
            }
            let mut name = str_prop(di, &mut dd, &DEVPKEY_Device_FriendlyName, false);
            if name.is_empty() {
                name = str_prop(di, &mut dd, &DEVPKEY_Device_DeviceDesc, false);
            }
            if inst.starts_with("BTHENUM\\DEV_") || inst.starts_with("BTHLE\\DEV_") {
                container_name.insert(cid.clone(), name.clone());
                let mut b = [0u8; 4];
                if prop_raw(di, &mut dd, &CLASS_OF_DEVICE, &mut b) == Some(DEVPROP_TYPE_UINT32) {
                    container_cod.insert(cid.clone(), u32::from_le_bytes(b));
                }
                let cat = str_prop(di, &mut dd, &DEVPKEY_DeviceContainer_Category, true);
                if !cat.is_empty() {
                    container_cat.insert(cid.clone(), cat);
                }
                let mut c = [0u8; 1];
                if prop_raw(di, &mut dd, &IS_CONNECTED, &mut c) == Some(DEVPROP_TYPE_BOOLEAN) {
                    // Klassischer und LE-Knoten können sich einen Container teilen: verbunden, wenn einer es ist.
                    let e = container_conn.entry(cid.clone()).or_insert(false);
                    *e = *e || c[0] != 0;
                }
            }
            let mut p = [0u8; 1];
            if prop_raw(di, &mut dd, &BT_BATTERY, &mut p) != Some(DEVPROP_TYPE_BYTE) {
                continue;
            }
            let mut c = [0xFFu8; 1];
            let conn = match prop_raw(di, &mut dd, &IS_CONNECTED, &mut c) {
                Some(t) if t == DEVPROP_TYPE_BOOLEAN => c[0] != 0,
                _ => true,
            };
            let (mut st, mut prob) = (0u32, 0u32);
            if CM_Get_DevNode_Status(&mut st, &mut prob, dd.DevInst, 0) == CR_SUCCESS && (st & DN_HAS_PROBLEM) != 0 {
                continue;
            }
            if p[0] > 100 {
                continue;
            }
            if let Some(prev) = battery.get(&cid) {
                if prev.connected && !conn {
                    continue; // lebenden Knoten bevorzugen
                }
            }
            battery.insert(cid, Found { pct: p[0], name: strip_service_suffix(&name), connected: conn });
        }
        SetupDiDestroyDeviceInfoList(di);
    }
    Scan { container_name, container_cod, container_cat, container_conn, battery }
}

/// Bluetooth-Geräte, für die Windows einen Akkustand kennt. `existing` sind bereits gefundene Geräte
/// (gleicher Name → nicht doppelt anzeigen).
pub fn poll(existing: &[DeviceStatus]) -> Vec<DeviceStatus> {
    let mut out = Vec::new();
    let Scan { container_name, container_cod, container_cat, container_conn, battery } = scan();
    for (cid, f) in battery {
        let connected = container_conn.get(&cid).copied().unwrap_or(f.connected);
        if !connected {
            continue;
        }
        let name = match container_name.get(&cid) {
            Some(n) if !n.is_empty() => n.clone(),
            _ if !f.name.is_empty() => f.name.clone(),
            _ => "Bluetooth device".into(),
        };
        if existing.iter().any(|e| e.via != "Bluetooth" && e.name.eq_ignore_ascii_case(&name)) {
            continue;
        }
        let mut kind = container_cod.get(&cid).map(|&c| kind_from_cod(c)).unwrap_or(DeviceKind::Other);
        if kind == DeviceKind::Other {
            if let Some(cat) = container_cat.get(&cid) {
                kind = kind_from_category(cat);
            }
        }
        if kind == DeviceKind::Other {
            kind = guess_kind(&name);
        }
        out.push(DeviceStatus { key: format!("bt:{cid}"), name, via: "Bluetooth".into(), kind, percent: f.pct as i32, ..Default::default() });
    }
    out
}
