//! Akku-Monitor – Akku-Stand von Funk-Headsets und -Mäusen direkt in der Windows-Taskleiste.
//! Rust-Port von BatteryHub (krisgfx9/Wireless-Device-Battery-Monitor, GPL-3.0).

#![windows_subsystem = "windows"]
#![allow(static_mut_refs)]
#![allow(dead_code)]

mod bluetooth;
mod brands;
mod update;
mod devices;
mod history;
mod i18n;
mod model;
mod rename;
mod util;
mod widget;

use brands::{accent_color, brand_of, Accent};
use i18n::{t, tf};
use model::{alerts, aliases, AliasMap, DeviceKind, DeviceStatus};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU32, Ordering};
use util::*;
use widget::WidgetItem;
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_TRAY: u32 = WM_APP + 1;
const WM_RESULTS: u32 = WM_APP + 2;
const WM_WIDGET_CLICK: u32 = WM_APP + 3;
const WM_UPDATE: u32 = WM_APP + 4;
const TIMER_DEBOUNCE: usize = 1;
const TIMER_WIDGET: usize = 2;
const TIMER_ALERT_ICON: usize = 3;
const TIMER_UPDATE: usize = 4;
const UPDATE_FIRST_MS: u32 = 60 * 1000;
const UPDATE_EVERY_MS: u32 = 6 * 60 * 60 * 1000;
const POLL_SECONDS: [u32; 8] = [30, 60, 120, 300, 600, 900, 1800, 3600];
const LOW_BATTERY: i32 = 15;
const PLACEHOLDER_ID: u32 = 1;
const ALERT_ICON_ID: u32 = 2;
const NIN_SELECT_: u32 = WM_USER;
const NIN_KEYSELECT_: u32 = WM_USER + 1;

const ID_REFRESH: u32 = 100;
const ID_AUTOSTART: u32 = 101;
const ID_HIDE_LAST_KNOWN: u32 = 102;
const ID_WIDGET_BACKGROUND: u32 = 103;
const ID_EXIT: u32 = 104;
const ID_MODE_WIDGET: u32 = 105;
const ID_MODE_TRAY: u32 = 106;
const ID_BRAND_COLORS: u32 = 109;
const ID_STACKED: u32 = 110;
const ID_SHOW_DISCONNECTED: u32 = 111;
const ID_RECORD_HISTORY: u32 = 112;
const ID_DIM_SLEEPING: u32 = 113;
const ID_UPDATE: u32 = 200;
const ID_AUTO_UPDATE: u32 = 201;
const ID_MONITOR_ALL: u32 = 300;
const ID_MONITOR_MAIN: u32 = 301;
const ID_MONITOR_FIRST: u32 = 302;
const ID_LANG_AUTO: u32 = 400;
const ID_LANG_ADD: u32 = 401;
const ID_LANG_FIRST: u32 = 402;
const ID_POLL_FIRST: u32 = 500;
const ID_DEVICE_FIRST: u32 = 1000;
const DEV_TOGGLE: u32 = 0;
const DEV_LEFT: u32 = 1;
const DEV_RIGHT: u32 = 2;
const DEV_RENAME: u32 = 3;
const DEV_RESET_NAME: u32 = 4;
const DEV_HISTORY: u32 = 5;
const DEV_ALERT_FIRST: u32 = 6;
const DEV_ALERT_DEFAULT: u32 = DEV_ALERT_FIRST + 8;
const DEV_ALERT_OFF: u32 = DEV_ALERT_DEFAULT + 1;
const DEV_STRIDE: u32 = 16;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "AkkuMonitor";
const WND_CLASS: &str = "AkkuMonitorWnd";
const MUTEX_NAME: &str = "Local\\AkkuMonitor.Instanz";

static POLL_INTERVAL_MS: AtomicU32 = AtomicU32::new(60_000);

#[derive(Default)]
struct TrayEntry {
    uid: u32,
    st: DeviceStatus,
    last_pct: i32,
    last_seen: u64,
    added: bool,
    low_warned: bool,
    icon_sig: String,
    icon: HICON,
}

struct App {
    inst: HINSTANCE,
    hwnd: HWND,
    refresh_evt: HANDLE,
    stop_evt: HANDLE,
    taskbar_created_msg: u32,
    light_taskbar: bool,
    want_widget: bool,
    use_widget: bool,
    hide_last_known: bool,
    show_disconnected: bool,
    dim_sleeping: bool,
    record_history: bool,
    widget_monitor: String,
    widget_background: bool,
    stacked: bool,
    brand_colors: bool,
    entries: BTreeMap<String, TrayEntry>,
    alert_latches: BTreeMap<String, bool>,
    alert_icon_added: bool,
    placeholder: bool,
    placeholder_icon: HICON,
    aliases: AliasMap,
    hidden: BTreeSet<String>,
    order: Vec<String>,
    alert_overrides: BTreeMap<String, i32>,
    demo: bool,
    auto_update: bool,
    mutex: HANDLE,
    update_busy: bool,
    worker: Option<std::thread::JoinHandle<()>>,
}

static mut APP: Option<App> = None;

fn app() -> &'static mut App {
    // Nur vom UI-Thread benutzt (wie die globalen Variablen im Original).
    unsafe { (*(&raw mut APP)).as_mut().unwrap() }
}

// ---------- Einstellungen ----------

fn display_name(e: &TrayEntry) -> String {
    app().aliases.get(&e.st.key).cloned().unwrap_or_else(|| e.st.name.clone())
}

fn load_aliases() {
    app().aliases = aliases::decode(&reg_get_multi(SETTINGS_KEY, "DeviceAliases"));
}

fn save_aliases() {
    reg_set_multi(SETTINGS_KEY, "DeviceAliases", &aliases::encode(&app().aliases));
}

fn stable_id(key: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for c in key.encode_utf16() {
        h ^= c as u32;
        h = h.wrapping_mul(16777619);
    }
    (h & 0x7FFFFFFF) | 0x100
}

fn apply_menu_theme(light: bool) {
    unsafe {
        let name = w("uxtheme.dll");
        let ux = LoadLibraryExW(name.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
        if ux.is_null() {
            return;
        }
        if let Some(f) = GetProcAddress(ux, 135 as *const u8) {
            let set_mode: extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set_mode(if light { 3 } else { 2 });
        }
        if let Some(f) = GetProcAddress(ux, 136 as *const u8) {
            let flush: extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}

fn load_hidden() {
    app().hidden = reg_get_multi(SETTINGS_KEY, "HiddenDevices").into_iter().collect();
}

fn save_hidden() {
    let v: Vec<String> = app().hidden.iter().cloned().collect();
    reg_set_multi(SETTINGS_KEY, "HiddenDevices", &v);
}

fn is_hidden(key: &str) -> bool {
    app().hidden.contains(key)
}

fn load_order() {
    app().order = reg_get_multi(SETTINGS_KEY, "DeviceOrder");
}

fn save_order() {
    let a = app();
    a.order.truncate(64);
    reg_set_multi(SETTINGS_KEY, "DeviceOrder", &a.order);
}

fn is_entry_hidden(e: &TrayEntry) -> bool {
    let a = app();
    if is_hidden(&e.st.key) {
        return true;
    }
    if a.show_disconnected && !e.st.connected {
        return false;
    }
    a.hide_last_known && (e.st.asleep || (e.st.percent < 0 && e.last_pct >= 0))
}

fn should_dim(s: &DeviceStatus) -> bool {
    if !s.connected {
        return true;
    }
    if s.asleep && !app().dim_sleeping {
        return false;
    }
    s.percent < 0
}

fn load_alert_thresholds() {
    let a = app();
    a.alert_overrides.clear();
    for item in reg_get_multi(SETTINGS_KEY, "AlertThresholds") {
        if let Some(sep) = item.rfind('\t') {
            if sep == 0 {
                continue;
            }
            if let Ok(n) = item[sep + 1..].parse::<i32>() {
                if (-1..=40).contains(&n) && alerts::valid_override(n) {
                    a.alert_overrides.insert(item[..sep].to_string(), n);
                }
            }
        }
    }
}

fn save_alert_thresholds() {
    let v: Vec<String> = app().alert_overrides.iter().map(|(k, v)| format!("{k}\t{v}")).collect();
    reg_set_multi(SETTINGS_KEY, "AlertThresholds", &v);
}

fn load_widget_monitor() -> String {
    match reg_get_string(SETTINGS_KEY, "WidgetMonitor") {
        Some(v) if !v.is_empty() => v,
        _ => if load_bool("OtherTaskbars", true) { "all".into() } else { "main".into() },
    }
}

fn set_widget_monitor(v: &str) {
    app().widget_monitor = v.to_string();
    reg_set_string(SETTINGS_KEY, "WidgetMonitor", v);
    widget::set_monitor_choice(v);
}

fn load_poll_seconds() -> u32 {
    reg_get_dword(SETTINGS_KEY, "PollIntervalSeconds").unwrap_or(60).clamp(30, 24 * 60 * 60)
}

fn autostart_value() -> String {
    format!("\"{}\"", exe_path())
}

fn autostart_enabled() -> bool {
    reg_get_string(RUN_KEY, RUN_VALUE).map(|v| v.eq_ignore_ascii_case(&autostart_value())).unwrap_or(false)
}

fn set_autostart(on: bool) {
    if on {
        reg_set_string(RUN_KEY, RUN_VALUE, &autostart_value());
    } else {
        let (k, n) = (w(RUN_KEY), w(RUN_VALUE));
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr()) };
    }
}

// ---------- Tray-Symbole zeichnen ----------
// Prozentzahl in großen Ziffern mit kleinem Füllstandsbalken darunter. GDI zeichnet den Text
// weiß auf schwarz; die Helligkeit wird zum Alphakanal (glatte Kanten auf transparentem Grund).

type Rgb = (u8, u8, u8);

fn render_icon(text: &str, bar_pct: i32, text_color: Rgb, bar_color: Rgb, dim: bool) -> HICON {
    unsafe {
        let tw = w("Shell_TrayWnd");
        let taskbar = FindWindowW(tw.as_ptr(), std::ptr::null());
        let d = if taskbar.is_null() { 0 } else { GetDpiForWindow(taskbar) };
        let s = GetSystemMetricsForDpi(SM_CXSMICON, if d != 0 { d } else { GetDpiForSystem() });
        let bar_h = 2.max(s / 8);
        let text_h = s - bar_h - 1;
        let wtext: Vec<u16> = text.encode_utf16().collect();
        let face = w("Segoe UI");

        let dc = CreateCompatibleDC(std::ptr::null_mut());
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        let (sw, sh) = (s * 3, s * 2);
        bi.bmiHeader.biWidth = sw;
        bi.bmiHeader.biHeight = -sh;
        let mut sbits: *mut core::ffi::c_void = std::ptr::null_mut();
        let scratch = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut sbits, std::ptr::null_mut(), 0);
        let old_bmp = SelectObject(dc, scratch as _);
        SetBkMode(dc, TRANSPARENT as _);
        SetTextColor(dc, 0x00FFFFFF);
        let sp = std::slice::from_raw_parts_mut(sbits as *mut u32, (sw * sh) as usize);

        let (mut bx0, mut by0, mut bx1, mut by1) = (0, 0, -1, -1);
        let mut fh = text_h * 2;
        while fh >= 6 {
            let f = CreateFontW(-fh, 0, 0, 0, FW_BOLD as i32, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_DEFAULT_PRECIS as u32,
                                CLIP_DEFAULT_PRECIS as u32, ANTIALIASED_QUALITY as u32, DEFAULT_PITCH as u32, face.as_ptr());
            let of = SelectObject(dc, f as _);
            SetTextCharacterExtra(dc, if wtext.len() >= 3 { -1 } else { 0 });
            sp.fill(0);
            TextOutW(dc, 2, 0, wtext.as_ptr(), wtext.len() as i32);
            GdiFlush();
            SelectObject(dc, of);
            DeleteObject(f as _);
            bx0 = sw;
            by0 = sh;
            bx1 = -1;
            by1 = -1;
            for y in 0..sh {
                for x in 0..sw {
                    if sp[(y * sw + x) as usize] & 0xFF != 0 {
                        bx0 = bx0.min(x);
                        bx1 = bx1.max(x);
                        by0 = by0.min(y);
                        by1 = by1.max(y);
                    }
                }
            }
            if bx1 < 0 {
                break;
            }
            if bx1 - bx0 + 1 <= s && by1 - by0 + 1 <= text_h {
                break;
            }
            fh -= 1;
        }

        bi.bmiHeader.biWidth = s;
        bi.bmiHeader.biHeight = -s;
        let mut ibits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut ibits, std::ptr::null_mut(), 0);
        let ip = std::slice::from_raw_parts_mut(ibits as *mut u32, (s * s) as usize);
        ip.fill(0);
        let alpha_scale = if dim { 140 } else { 255 };
        let mut put = |x: i32, y: i32, c: Rgb, a: i32| {
            if x < 0 || y < 0 || x >= s || y >= s || a <= 0 {
                return;
            }
            let a = a * alpha_scale / 255;
            ip[(y * s + x) as usize] = (a as u32) << 24 | (c.0 as u32) << 16 | (c.1 as u32) << 8 | c.2 as u32;
        };
        if bx1 >= 0 {
            let (gw, gh) = (bx1 - bx0 + 1, by1 - by0 + 1);
            let (ox, oy) = ((s - gw) / 2, (text_h - gh) / 2);
            for y in 0..gh {
                for x in 0..gw {
                    put(ox + x, oy + y, text_color, (sp[((by0 + y) * sw + bx0 + x) as usize] & 0xFF) as i32);
                }
            }
        }
        let fill = if bar_pct < 0 { 0 } else { (if bar_pct > 0 { 1 } else { 0 }).max((bar_pct * s + 50) / 100) };
        let track = if app().light_taskbar { (0, 0, 0) } else { (255, 255, 255) };
        for y in s - bar_h..s {
            for x in 0..s {
                if x < fill {
                    put(x, y, bar_color, 255);
                } else {
                    put(x, y, track, 70);
                }
            }
        }
        SelectObject(dc, old_bmp);
        DeleteObject(scratch as _);
        DeleteDC(dc);

        let mask = CreateBitmap(s, s, 1, 1, std::ptr::null());
        let mdc = CreateCompatibleDC(std::ptr::null_mut());
        let o = SelectObject(mdc, mask as _);
        PatBlt(mdc, 0, 0, s, s, BLACKNESS);
        SelectObject(mdc, o);
        DeleteDC(mdc);
        let ii = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&ii);
        DeleteObject(mask as _);
        DeleteObject(color as _);
        icon
    }
}

fn foreground() -> Rgb {
    if app().light_taskbar { (0, 0, 0) } else { (255, 255, 255) }
}

fn accent_for(s: &DeviceStatus) -> Accent {
    if app().brand_colors { brand_of(s) } else { Accent::None }
}

fn icon_for(pct: i32, charging: bool, dim: bool, accent: Accent) -> HICON {
    let fg = foreground();
    let bc = accent_color(accent, app().light_taskbar);
    let bar = if charging {
        (0, 170, 255)
    } else if pct <= LOW_BATTERY {
        (232, 32, 48)
    } else if pct <= 30 {
        (255, 150, 0)
    } else if accent != Accent::None {
        bc
    } else {
        (30, 190, 90)
    };
    let text = if !charging && pct >= 0 && pct <= LOW_BATTERY { (240, 50, 60) } else { fg };
    let t = if pct < 0 { "?".to_string() } else { pct.to_string() };
    render_icon(&t, pct, text, bar, dim)
}

// ---------- Tray-Verwaltung ----------

fn status_text(e: &TrayEntry) -> String {
    let s = &e.st;
    if !s.connected {
        return t("off / not connected");
    }
    let mut r = if !s.level_text.is_empty() && s.percent >= 0 {
        t(&s.level_text)
    } else if s.percent >= 0 {
        format!("{}%", s.percent)
    } else if e.last_pct >= 0 {
        tf("{percent}% (last known)", &[("percent", e.last_pct.to_string())])
    } else {
        t("unknown")
    };
    if !s.detail.is_empty() && s.percent >= 0 {
        r += &format!(" ({})", s.detail);
    }
    if s.full || (s.charging && s.percent >= 100) {
        r += &format!(", {}", t("fully charged"));
    } else if s.charging {
        r += &format!(", {}", t("charging"));
    }
    if s.docked {
        r += &format!(", {}", t("in the dock"));
    }
    if s.asleep {
        r += &format!(", {}", t("asleep"));
    }
    r
}

fn copy_tip(dst: &mut [u16], s: &str) {
    let v: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..v.len()].copy_from_slice(&v);
    dst[v.len()] = 0;
}

fn tray_call(msg: NOTIFY_ICON_MESSAGE, uid: u32, icon: HICON, tip: &str, info: Option<(&str, &str)>) {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = app().hwnd;
        nid.uID = uid;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = icon;
        copy_tip(&mut nid.szTip, tip);
        if let Some((title, text)) = info {
            nid.uFlags |= NIF_INFO;
            copy_tip(&mut nid.szInfoTitle, title);
            copy_tip(&mut nid.szInfo, text);
            nid.dwInfoFlags = NIIF_WARNING;
        }
        Shell_NotifyIconW(msg, &nid);
        if msg == NIM_ADD {
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            Shell_NotifyIconW(NIM_SETVERSION, &nid);
        }
    }
}

fn tray_delete(uid: u32) {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = app().hwnd;
        nid.uID = uid;
        Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn placeholder_icon() -> HICON {
    let a = app();
    if a.placeholder_icon.is_null() {
        a.placeholder_icon = render_icon("-", -1, foreground(), foreground(), true);
    }
    a.placeholder_icon
}

/// Windows-Benachrichtigung über das (kurzzeitige) Hinweis-Symbol.
fn notify(title: &str, message: &str) {
    let a = app();
    let icon = placeholder_icon();
    tray_call(if a.alert_icon_added { NIM_MODIFY } else { NIM_ADD }, ALERT_ICON_ID, icon, APP_NAME, Some((title, message)));
    a.alert_icon_added = true;
    unsafe { SetTimer(a.hwnd, TIMER_ALERT_ICON, 60 * 1000, None) };
}

/// Update-Prüfung im Hintergrund starten (manuell = mit Rückmeldung).
fn start_update_check(manual: bool) {
    let a = app();
    if a.update_busy {
        return;
    }
    a.update_busy = true;
    let hwnd = a.hwnd as isize;
    std::thread::spawn(move || {
        let r = update::check_and_download();
        let ptr = Box::into_raw(Box::new((manual, r)));
        if unsafe { PostMessageW(hwnd as HWND, WM_UPDATE, 0, ptr as LPARAM) } == 0 {
            drop(unsafe { Box::from_raw(ptr) });
        }
    });
}

fn on_update_result(manual: bool, r: update::CheckResult) {
    let a = app();
    a.update_busy = false;
    match r {
        update::CheckResult::Ready(version, path) => {
            if !manual && (!a.auto_update || !update::is_installed()) {
                return;
            }
            // Einzelinstanz-Sperre freigeben, sonst meldet der Installer die App als „noch offen“.
            if !a.mutex.is_null() {
                unsafe {
                    ReleaseMutex(a.mutex);
                    CloseHandle(a.mutex);
                }
                a.mutex = std::ptr::null_mut();
            }
            if update::launch_installer(&path) {
                // Der Installer ersetzt die Exe und startet die App danach neu.
                unsafe { DestroyWindow(a.hwnd) };
            } else {
                notify(&t("Akku-Monitor update"), &tf("Update available: {version}...", &[("version", version)]));
            }
        }
        update::CheckResult::UpToDate => {
            if manual {
                notify(APP_NAME, &tf("You're up to date ({version}).", &[("version", update::current_version().to_string())]));
            }
        }
        update::CheckResult::Failed(_) => {
            if manual {
                notify(APP_NAME, &t("Could not check for updates. Check your Internet connection and try again."));
            }
        }
    }
}

fn show_battery_alert(key: &str) {
    let a = app();
    let Some(e) = a.entries.get(key) else { return };
    let title = t("Low battery");
    let message = tf("{name} is at {percent}%", &[("name", display_name(e)), ("percent", e.st.percent.to_string())]);
    if !a.use_widget && e.added {
        tray_call(NIM_MODIFY, e.uid, e.icon, &format!("{}\n{}", display_name(e), status_text(e)), Some((&title, &message)));
        return;
    }
    // Windows-Benachrichtigungen brauchen ein Tray-Symbol: im Widget-Modus kurz eines anlegen.
    let icon = placeholder_icon();
    tray_call(if a.alert_icon_added { NIM_MODIFY } else { NIM_ADD }, ALERT_ICON_ID, icon, APP_NAME, Some((&title, &message)));
    a.alert_icon_added = true;
    unsafe { SetTimer(a.hwnd, TIMER_ALERT_ICON, 60 * 1000, None) };
}

fn update_placeholder() {
    let a = app();
    if a.use_widget {
        if a.placeholder {
            tray_delete(PLACEHOLDER_ID);
            a.placeholder = false;
        }
        return;
    }
    let any = a.entries.values().any(|e| e.added);
    if !any && !a.placeholder {
        let icon = placeholder_icon();
        tray_call(NIM_ADD, PLACEHOLDER_ID, icon, &format!("{APP_NAME}\n{}", t("No devices with battery info found")), None);
        a.placeholder = true;
    } else if any && a.placeholder {
        tray_delete(PLACEHOLDER_ID);
        a.placeholder = false;
    }
}

fn apply_entry(key: &str) {
    let a = app();
    let Some(e) = a.entries.get_mut(key) else { return };
    let visible = !a.use_widget && !is_entry_hidden(e) && (e.st.connected || a.show_disconnected);
    if !visible {
        if e.added {
            tray_delete(e.uid);
            e.added = false;
        }
        return;
    }
    let s = e.st.clone();
    let shown = if !s.connected { -1 } else if s.percent >= 0 { s.percent } else { e.last_pct };
    let dim = should_dim(&s);
    let charging = s.connected && !s.asleep && s.charging && !s.full && shown < 100;
    let accent = accent_for(&s);
    let sig = format!("{shown}{}{}{}a{:?}{}", if charging { "c" } else { "" }, if dim { "d" } else { "" },
                      if a.light_taskbar { "l" } else { "k" }, accent, if s.connected { "|on" } else { "|off" });
    if sig != e.icon_sig || e.icon.is_null() {
        if !e.icon.is_null() {
            unsafe { DestroyIcon(e.icon) };
        }
        e.icon = if s.connected { icon_for(shown, charging, dim, accent) } else { render_icon("\u{00D7}", -1, foreground(), foreground(), true) };
        e.icon_sig = sig;
    }
    let tip = format!("{}\n{}", display_name(e), status_text(e));
    tray_call(if e.added { NIM_MODIFY } else { NIM_ADD }, e.uid, e.icon, &tip, None);
    e.added = true;
}

fn kind_order(k: DeviceKind) -> i32 {
    match k {
        DeviceKind::Headset => 0,
        DeviceKind::Mouse => 1,
        DeviceKind::Keyboard => 2,
        DeviceKind::Gamepad => 3,
        DeviceKind::Phone => 4,
        _ => 5,
    }
}

fn is_shown(e: &TrayEntry) -> bool {
    (e.st.connected || app().show_disconnected) && !is_entry_hidden(e)
}

/// Geräteschlüssel in Anzeigereihenfolge: erst die eigene Reihenfolge, dann nach Art und Name.
fn ordered_keys(shown_only: bool) -> Vec<String> {
    let a = app();
    let mut list: Vec<&TrayEntry> = a.entries.values().filter(|e| !shown_only || is_shown(e)).collect();
    let rank = |e: &TrayEntry| a.order.iter().position(|k| *k == e.st.key).unwrap_or(usize::MAX);
    list.sort_by(|x, y| {
        rank(x).cmp(&rank(y))
            .then(kind_order(x.st.kind).cmp(&kind_order(y.st.kind)))
            .then(display_name(x).cmp(&display_name(y)))
            .then(x.st.key.cmp(&y.st.key))
    });
    list.into_iter().map(|e| e.st.key.clone()).collect()
}

fn update_widget() {
    let a = app();
    if !a.use_widget {
        return;
    }
    let keys = ordered_keys(true);
    let label_in: Vec<(DeviceKind, String)> = keys.iter().map(|k| (a.entries[k].st.kind, display_name(&a.entries[k]))).collect();
    let labels = model::short_labels(&label_in, 3);
    let mut items = Vec::new();
    let mut tip = String::new();
    for (i, k) in keys.iter().enumerate() {
        let e = &a.entries[k];
        let offline = !e.st.connected;
        let mut it = WidgetItem { kind: e.st.kind, ..Default::default() };
        it.percent = if offline { -1 } else if e.st.percent >= 0 { e.st.percent } else { e.last_pct };
        if offline {
            it.text = "\u{00D7}".into();
        } else if e.st.percent >= 0 && !e.st.level_text.is_empty() {
            it.text = t(&e.st.level_text);
        }
        it.charging = !offline && !e.st.asleep && e.st.charging;
        it.full = !offline && (e.st.full || (e.st.charging && it.percent >= 100));
        it.docked = !offline && e.st.docked;
        it.dim = should_dim(&e.st);
        it.low = !offline && !it.charging && it.percent >= 0 && it.percent <= LOW_BATTERY;
        it.bluetooth = e.st.via == "Bluetooth";
        it.label = labels[i].clone();
        it.accent = accent_for(&e.st);
        items.push(it);
        if !tip.is_empty() {
            tip.push('\n');
        }
        tip += &format!("{}: {}", display_name(e), status_text(e));
    }
    if tip.is_empty() {
        tip = format!("{APP_NAME}: {}", t("No devices with battery info found"));
    }
    widget::set_items(items, &tip);
}

fn on_results(results: Vec<DeviceStatus>) {
    let a = app();
    let mut seen = BTreeSet::new();
    let now = tick();
    for s in results {
        let key = s.key.clone();
        let e = a.entries.entry(key.clone()).or_insert_with(|| TrayEntry { last_pct: -1, ..Default::default() });
        if e.uid == 0 {
            e.uid = stable_id(&key);
        }
        e.st = s.clone();
        if s.percent >= 0 {
            e.last_pct = s.percent;
        }
        e.last_seen = now;
        seen.insert(key.clone());
        if let Some(&latch) = a.alert_latches.get(&key) {
            e.low_warned = latch;
        }
        let threshold = alerts::effective_threshold(&a.alert_overrides, &key);
        let mut warned = e.low_warned;
        let notify = !is_hidden(&key) && alerts::should_notify(&s, threshold, &mut warned);
        e.low_warned = warned;
        a.alert_latches.insert(key.clone(), warned);
        if a.alert_latches.len() > 256 {
            let first = a.alert_latches.keys().next().cloned().unwrap();
            a.alert_latches.remove(&first);
        }
        if a.record_history && s.connected && !s.asleep && s.percent >= 0 {
            history::observe(&key, s.percent, s.charging);
        }
        apply_entry(&key);
        if notify {
            show_battery_alert(&key);
        }
    }
    let poll_ms = POLL_INTERVAL_MS.load(Ordering::Relaxed) as u64;
    let keys: Vec<String> = a.entries.keys().cloned().collect();
    for k in keys {
        if seen.contains(&k) {
            continue;
        }
        let e = a.entries.get_mut(&k).unwrap();
        // Manche Empfänger melden schlafende Geräte ab: den letzten Wert eine Weile weiter zeigen.
        let in_grace = e.st.grace_sec > 0 && e.st.connected && now - e.last_seen < (e.st.grace_sec as u64 * 1000).max(2 * poll_ms);
        if in_grace {
            e.st.percent = -1;
            e.st.asleep = true;
            apply_entry(&k);
        } else {
            let e = a.entries.remove(&k).unwrap();
            if e.added {
                tray_delete(e.uid);
            }
            if !e.icon.is_null() {
                unsafe { DestroyIcon(e.icon) };
            }
        }
    }
    update_placeholder();
    update_widget();
}

/// Anzeige an den gewählten Modus angleichen; fällt auf Tray-Symbole zurück, wenn das Widget
/// (noch) nicht angelegt werden kann. Einmal pro Sekunde.
fn reconcile() {
    let a = app();
    let mut alive = a.want_widget && widget::alive();
    if a.want_widget && !alive {
        alive = widget::create(a.inst, a.hwnd, WM_WIDGET_CLICK, a.light_taskbar);
    }
    if !a.want_widget {
        widget::destroy();
    }
    let changed = alive != a.use_widget;
    a.use_widget = alive;
    if changed {
        for k in a.entries.keys().cloned().collect::<Vec<_>>() {
            apply_entry(&k);
        }
        update_placeholder();
        update_widget();
    } else if a.use_widget {
        widget::reposition();
    }
}

fn apply_all() {
    for k in app().entries.keys().cloned().collect::<Vec<_>>() {
        apply_entry(&k);
    }
}

fn redraw_all() {
    let a = app();
    if a.alert_icon_added {
        unsafe { KillTimer(a.hwnd, TIMER_ALERT_ICON) };
        tray_delete(ALERT_ICON_ID);
        a.alert_icon_added = false;
    }
    if !a.placeholder_icon.is_null() {
        unsafe { DestroyIcon(a.placeholder_icon) };
        a.placeholder_icon = std::ptr::null_mut();
        if a.placeholder {
            tray_delete(PLACEHOLDER_ID);
            a.placeholder = false;
        }
    }
    for e in a.entries.values_mut() {
        e.icon_sig.clear();
    }
    apply_all();
    update_placeholder();
}

fn toggle_hidden(key: &str) {
    let a = app();
    if !a.hidden.remove(key) {
        a.hidden.insert(key.to_string());
    }
    save_hidden();
    apply_entry(key);
    update_placeholder();
    update_widget();
}

fn rename_device(key: &str) {
    let a = app();
    let Some(e) = a.entries.get(key) else { return };
    let original = e.st.name.clone();
    let Some(chosen) = rename::prompt_device_name(a.hwnd, &display_name(e)) else { return };
    if chosen == original {
        a.aliases.remove(key);
    } else if a.aliases.contains_key(key) || a.aliases.len() < aliases::MAX_ALIASES {
        a.aliases.insert(key.to_string(), chosen);
    } else {
        return;
    }
    save_aliases();
    apply_entry(key);
    update_widget();
}

fn reset_device_name(key: &str) {
    if app().aliases.remove(key).is_none() {
        return;
    }
    save_aliases();
    apply_entry(key);
    update_widget();
}

fn set_alert_override(key: &str, threshold: i32) {
    let a = app();
    if threshold == alerts::USE_DEFAULT {
        a.alert_overrides.remove(key);
    } else if alerts::valid_override(threshold) {
        a.alert_overrides.insert(key.to_string(), threshold);
    } else {
        return;
    }
    save_alert_thresholds();
    a.alert_latches.remove(key);
    if let Some(e) = a.entries.get_mut(key) {
        e.low_warned = false;
    }
    unsafe { SetEvent(a.refresh_evt) };
}

fn move_entry(key: &str, dir: i32) {
    let a = app();
    let mut list = ordered_keys(true);
    let Some(i) = list.iter().position(|k| k == key) else { return };
    let j = i as i32 + dir;
    if j < 0 || j >= list.len() as i32 {
        return;
    }
    list.swap(i, j as usize);
    for k in &a.order {
        if !list.contains(k) {
            list.push(k.clone());
        }
    }
    a.order = list;
    save_order();
    update_widget();
}

// ---------- Menü ----------

fn append(m: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str) {
    let tw = w(text);
    unsafe { AppendMenuW(m, flags, id, tw.as_ptr()) };
}

fn separator(m: HMENU) {
    unsafe { AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null()) };
}

fn checked(on: bool) -> MENU_ITEM_FLAGS {
    if on { MF_CHECKED } else { 0 }
}

fn grayed(on: bool) -> MENU_ITEM_FLAGS {
    if on { MF_GRAYED } else { 0 }
}

fn show_menu() {
    let a = app();
    apply_menu_theme(a.light_taskbar);
    unsafe {
        let m = CreatePopupMenu();
        let all = ordered_keys(false);
        let shown = ordered_keys(true);
        let mut keys: Vec<String> = Vec::new();
        if all.is_empty() {
            append(m, MF_STRING | MF_GRAYED, 0, &t("No devices with battery info found"));
        } else {
            append(m, MF_STRING | MF_GRAYED, 0, &t("Point at a device to show, hide or move it"));
        }
        for k in &all {
            let e = &a.entries[k];
            let base = ID_DEVICE_FIRST + keys.len() as u32 * DEV_STRIDE;
            let pos = shown.iter().position(|s| s == k);
            let can_left = matches!(pos, Some(p) if p > 0);
            let can_right = matches!(pos, Some(p) if p + 1 < shown.len());
            let sub = CreatePopupMenu();
            append(sub, MF_STRING | checked(!is_hidden(k)), (base + DEV_TOGGLE) as usize,
                   &if a.want_widget { t("Show on the taskbar") } else { t("Show in the tray") });
            separator(sub);
            append(sub, MF_STRING | grayed(!can_left), (base + DEV_LEFT) as usize, &if a.want_widget { t("Move left") } else { t("Move up") });
            append(sub, MF_STRING | grayed(!can_right), (base + DEV_RIGHT) as usize, &if a.want_widget { t("Move right") } else { t("Move down") });
            separator(sub);
            append(sub, MF_STRING, (base + DEV_RENAME) as usize, &t("Rename device..."));
            append(sub, MF_STRING | grayed(!a.aliases.contains_key(k)), (base + DEV_RESET_NAME) as usize, &t("Reset to original name"));
            let alert_menu = CreatePopupMenu();
            let sel = alerts::override_for(&a.alert_overrides, k);
            append(alert_menu, MF_STRING | checked(sel == alerts::USE_DEFAULT), (base + DEV_ALERT_DEFAULT) as usize, &t("Use default (15%)"));
            append(alert_menu, MF_STRING | checked(sel == alerts::DISABLED), (base + DEV_ALERT_OFF) as usize, &t("Disable notifications"));
            separator(alert_menu);
            for (i, level) in alerts::THRESHOLDS.iter().enumerate() {
                append(alert_menu, MF_STRING | checked(sel == *level), (base + DEV_ALERT_FIRST + i as u32) as usize, &format!("{level}%"));
            }
            append(sub, MF_POPUP, alert_menu as usize, &t("Battery alerts"));
            append(sub, MF_STRING, (base + DEV_HISTORY) as usize, &t("Battery history..."));
            let line = format!("{}:  {}   [{}]", display_name(e), status_text(e), t(&e.st.via));
            append(m, MF_POPUP | checked(!is_hidden(k)), sub as usize, &line);
            keys.push(k.clone());
        }
        append(m, MF_STRING | checked(a.hide_last_known), ID_HIDE_LAST_KNOWN as usize, &t("Hide last known / asleep devices"));
        append(m, MF_STRING | checked(a.show_disconnected), ID_SHOW_DISCONNECTED as usize, &t("Show disconnected devices"));
        append(m, MF_STRING | checked(a.dim_sleeping), ID_DIM_SLEEPING as usize, &t("Dim sleeping devices"));
        append(m, MF_STRING | checked(a.record_history), ID_RECORD_HISTORY as usize, &t("Record battery history"));
        separator(m);
        append(m, MF_STRING | checked(a.want_widget), ID_MODE_WIDGET as usize, &t("Show on the taskbar (icon + percentage)"));
        append(m, MF_STRING | checked(!a.want_widget), ID_MODE_TRAY as usize, &t("Show as tray icons"));
        CheckMenuRadioItem(m, ID_MODE_WIDGET, ID_MODE_TRAY, if a.want_widget { ID_MODE_WIDGET } else { ID_MODE_TRAY }, MF_BYCOMMAND);
        append(m, MF_STRING | checked(a.widget_background) | grayed(!a.want_widget), ID_WIDGET_BACKGROUND as usize,
               &t("Background behind taskbar text (for transparent taskbars)"));
        append(m, MF_STRING | checked(a.stacked) | grayed(!a.want_widget), ID_STACKED as usize, &t("Icons above percentages (narrower)"));
        let mut monitors = widget::other_monitors();
        monitors.truncate(31);
        if !monitors.is_empty() {
            let sub = CreatePopupMenu();
            let f = MF_STRING | grayed(!a.want_widget);
            append(sub, f, ID_MONITOR_ALL as usize, &t("All monitors"));
            append(sub, f, ID_MONITOR_MAIN as usize, &t("Main monitor only"));
            let mut chk = if a.widget_monitor == "main" { ID_MONITOR_MAIN } else { ID_MONITOR_ALL };
            let mut found = a.widget_monitor == "all" || a.widget_monitor == "main";
            for (i, mo) in monitors.iter().enumerate() {
                append(sub, f, (ID_MONITOR_FIRST + i as u32) as usize, &tf("{monitor} only", &[("monitor", mo.label.clone())]));
                if mo.id == a.widget_monitor {
                    chk = ID_MONITOR_FIRST + i as u32;
                    found = true;
                }
            }
            if !found {
                append(sub, MF_STRING | MF_GRAYED, (ID_MONITOR_FIRST + 31) as usize, &t("Chosen monitor (not connected, shown on main)"));
                chk = ID_MONITOR_FIRST + 31;
            }
            CheckMenuRadioItem(sub, ID_MONITOR_ALL, ID_MONITOR_FIRST + 31, chk, MF_BYCOMMAND);
            append(m, MF_POPUP | grayed(!a.want_widget), sub as usize, &t("Show the widget on"));
        }
        append(m, MF_STRING | checked(a.brand_colors), ID_BRAND_COLORS as usize, &t("Brand colors (Razer green, Logitech blue, ...)"));
        separator(m);
        i18n::init();
        let mut languages = i18n::available();
        languages.truncate(64);
        {
            let sub = CreatePopupMenu();
            append(sub, MF_STRING, ID_LANG_AUTO as usize, &t("Automatic (Windows language)"));
            separator(sub);
            let mut chk = ID_LANG_AUTO;
            let choice = i18n::choice();
            for (i, l) in languages.iter().enumerate() {
                append(sub, MF_STRING, (ID_LANG_FIRST + i as u32) as usize, &l.name);
                if !choice.is_empty() && l.code.eq_ignore_ascii_case(&choice) {
                    chk = ID_LANG_FIRST + i as u32;
                }
            }
            CheckMenuRadioItem(sub, ID_LANG_AUTO, ID_LANG_FIRST + 63, chk, MF_BYCOMMAND);
            separator(sub);
            append(sub, MF_STRING, ID_LANG_ADD as usize, &t("Add your own language..."));
            append(m, MF_POPUP, sub as usize, &t("Language"));
        }
        {
            let sub = CreatePopupMenu();
            let mut chk = 0;
            let cur = POLL_INTERVAL_MS.load(Ordering::Relaxed);
            for (i, s) in POLL_SECONDS.iter().enumerate() {
                let lbl = if *s < 60 { tf("{seconds} s", &[("seconds", s.to_string())]) } else { tf("{minutes} min", &[("minutes", (s / 60).to_string())]) };
                append(sub, MF_STRING, (ID_POLL_FIRST + i as u32) as usize, &lbl);
                if s * 1000 == cur {
                    chk = ID_POLL_FIRST + i as u32;
                }
            }
            if chk != 0 {
                CheckMenuRadioItem(sub, ID_POLL_FIRST, ID_POLL_FIRST + POLL_SECONDS.len() as u32 - 1, chk, MF_BYCOMMAND);
            }
            append(m, MF_POPUP, sub as usize, &t("Check battery every"));
        }
        append(m, MF_STRING, ID_REFRESH as usize, &t("Refresh now"));
        append(m, MF_STRING | checked(autostart_enabled()), ID_AUTOSTART as usize, &t("Start with Windows"));
        separator(m);
        append(m, MF_STRING | grayed(a.update_busy), ID_UPDATE as usize,
               &if a.update_busy { t("Checking for updates...") } else { t("Check for updates...") });
        append(m, MF_STRING | checked(a.auto_update && update::is_installed()) | grayed(!update::is_installed()), ID_AUTO_UPDATE as usize,
               &t("Automatically check for updates"));
        separator(m);
        append(m, MF_STRING, ID_EXIT as usize, &t("Exit"));

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(a.hwnd);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | if i18n::rtl() { TPM_LAYOUTRTL } else { 0 },
                                 pt.x, pt.y, 0, a.hwnd, std::ptr::null()) as u32;
        PostMessageW(a.hwnd, WM_NULL, 0, 0);
        DestroyMenu(m);
        match cmd {
            0 => {}
            ID_REFRESH => {
                SetEvent(a.refresh_evt);
            }
            ID_AUTOSTART => set_autostart(!autostart_enabled()),
            ID_HIDE_LAST_KNOWN => {
                a.hide_last_known = !a.hide_last_known;
                save_bool("HideLastKnownDevices", a.hide_last_known);
                apply_all();
                update_placeholder();
                update_widget();
            }
            ID_SHOW_DISCONNECTED => {
                a.show_disconnected = !a.show_disconnected;
                save_bool("ShowDisconnectedDevices", a.show_disconnected);
                apply_all();
                update_placeholder();
                update_widget();
            }
            ID_DIM_SLEEPING => {
                a.dim_sleeping = !a.dim_sleeping;
                save_bool("DimSleepingDevices", a.dim_sleeping);
                apply_all();
                update_widget();
            }
            ID_RECORD_HISTORY => {
                a.record_history = !a.record_history;
                save_bool("RecordBatteryHistory", a.record_history);
            }
            ID_WIDGET_BACKGROUND => {
                a.widget_background = !a.widget_background;
                save_bool("WidgetBackground", a.widget_background);
                widget::set_background(a.widget_background);
            }
            ID_MONITOR_ALL => set_widget_monitor("all"),
            ID_MONITOR_MAIN => set_widget_monitor("main"),
            ID_LANG_AUTO => set_language(""),
            ID_LANG_ADD => i18n::open_user_folder(),
            ID_BRAND_COLORS => {
                a.brand_colors = !a.brand_colors;
                save_bool("BrandColors", a.brand_colors);
                redraw_all();
                update_widget();
            }
            ID_STACKED => {
                a.stacked = !a.stacked;
                save_bool("StackedLayout", a.stacked);
                widget::set_stacked(a.stacked);
            }
            ID_MODE_WIDGET | ID_MODE_TRAY => {
                a.want_widget = cmd == ID_MODE_WIDGET;
                save_bool("TaskbarWidget", a.want_widget);
                reconcile();
            }
            ID_UPDATE => start_update_check(true),
            ID_AUTO_UPDATE => {
                a.auto_update = !a.auto_update;
                save_bool("AutoUpdate", a.auto_update);
                if a.auto_update {
                    start_update_check(false);
                }
            }
            ID_EXIT => {
                DestroyWindow(a.hwnd);
            }
            c if (ID_POLL_FIRST..ID_POLL_FIRST + POLL_SECONDS.len() as u32).contains(&c) => {
                let secs = POLL_SECONDS[(c - ID_POLL_FIRST) as usize];
                POLL_INTERVAL_MS.store(secs * 1000, Ordering::Relaxed);
                reg_set_dword(SETTINGS_KEY, "PollIntervalSeconds", secs);
                SetEvent(a.refresh_evt);
            }
            c if (ID_LANG_FIRST..ID_LANG_FIRST + languages.len() as u32).contains(&c) => {
                set_language(&languages[(c - ID_LANG_FIRST) as usize].code.clone());
            }
            c if (ID_MONITOR_FIRST..ID_MONITOR_FIRST + monitors.len() as u32).contains(&c) => {
                set_widget_monitor(&monitors[(c - ID_MONITOR_FIRST) as usize].id.clone());
            }
            c if c >= ID_DEVICE_FIRST && c < ID_DEVICE_FIRST + keys.len() as u32 * DEV_STRIDE => {
                let key = keys[((c - ID_DEVICE_FIRST) / DEV_STRIDE) as usize].clone();
                match (c - ID_DEVICE_FIRST) % DEV_STRIDE {
                    DEV_TOGGLE => toggle_hidden(&key),
                    DEV_LEFT => move_entry(&key, -1),
                    DEV_RIGHT => move_entry(&key, 1),
                    DEV_RENAME => rename_device(&key),
                    DEV_RESET_NAME => reset_device_name(&key),
                    DEV_ALERT_DEFAULT => set_alert_override(&key, alerts::USE_DEFAULT),
                    DEV_ALERT_OFF => set_alert_override(&key, alerts::DISABLED),
                    DEV_HISTORY => {
                        if let Some(e) = a.entries.get(&key) {
                            history::show(&key, &display_name(e));
                        }
                    }
                    slot if (DEV_ALERT_FIRST..DEV_ALERT_FIRST + alerts::THRESHOLDS.len() as u32).contains(&slot) => {
                        set_alert_override(&key, alerts::THRESHOLDS[(slot - DEV_ALERT_FIRST) as usize]);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn set_language(code: &str) {
    i18n::set_choice(code);
    redraw_all();
    update_widget();
}

// ---------- Arbeits-Thread ----------

fn demo_devices() -> Vec<DeviceStatus> {
    let mk = |key: &str, name: &str, kind: DeviceKind, pct: i32, vid: u16, charging: bool| DeviceStatus {
        key: key.into(), name: name.into(), kind, percent: pct, vid, charging, via: "demo".into(), ..Default::default()
    };
    let mut v = vec![
        mk("razer:demo", "Razer DeathAdder V3 Pro", DeviceKind::Mouse, 78, 0x1532, false),
        mk("demo:logi", "Logitech G PRO X 2 LIGHTSPEED", DeviceKind::Headset, 64, 0x046D, false),
        mk("steelseries:demo", "SteelSeries Arctis Nova 7", DeviceKind::Headset, 55, 0, false),
        mk("jabra:demo", "Jabra Evolve2 65", DeviceKind::Headset, 23, 0x0B0E, false),
        mk("coolermaster:demo", "Cooler Master MM712", DeviceKind::Mouse, 12, 0x2516, false),
        mk("steam:demo", "Steam Controller", DeviceKind::Gamepad, 50, 0x28DE, true),
        mk("bt:demo-plain", "Bluetooth Headphones", DeviceKind::Headset, 100, 0, true),
    ];
    let mut off = mk("demo:offline-receiver", "Offline wireless headset", DeviceKind::Headset, -1, 0, false);
    off.via = "2.4 GHz receiver".into();
    off.connected = false;
    v.push(off);
    v
}

fn worker(hwnd: isize, refresh: isize, stop: isize, demo: bool) {
    // Beim Anmelden kann ein Empfänger auftauchen, bevor seine Funkverbindung steht.
    let startup_delays = [3000u32, 7000, 15000];
    let mut startup = 0;
    let mut api = hidapi::HidApi::new().ok();
    let events = [stop as HANDLE, refresh as HANDLE];
    loop {
        let list = if demo { demo_devices() } else { api.as_mut().map(devices::poll_all).unwrap_or_default() };
        let ptr = Box::into_raw(Box::new(list));
        if unsafe { PostMessageW(hwnd as HWND, WM_RESULTS, 0, ptr as LPARAM) } == 0 {
            drop(unsafe { Box::from_raw(ptr) });
        }
        let mut delay = POLL_INTERVAL_MS.load(Ordering::Relaxed);
        if startup < startup_delays.len() {
            delay = startup_delays[startup];
            startup += 1;
        }
        if unsafe { WaitForMultipleObjects(2, events.as_ptr(), 0, delay) } == WAIT_OBJECT_0 {
            return;
        }
    }
}

fn refresh_soon(ms: u32) {
    unsafe { SetTimer(app().hwnd, TIMER_DEBOUNCE, ms, None) };
}

unsafe extern "system" fn wnd_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let a = app();
    if msg == a.taskbar_created_msg && msg != 0 {
        // Explorer neu gestartet
        widget::destroy();
        a.use_widget = false;
        a.alert_icon_added = false;
        unsafe { KillTimer(a.hwnd, TIMER_ALERT_ICON) };
        a.placeholder = false;
        for e in a.entries.values_mut() {
            e.added = false;
        }
        apply_all();
        update_placeholder();
        reconcile();
        return 0;
    }
    unsafe {
        match msg {
            WM_RESULTS => {
                let v = Box::from_raw(lp as *mut Vec<DeviceStatus>);
                on_results(*v);
                0
            }
            WM_TRAY => {
                match loword(lp as usize) {
                    WM_CONTEXTMENU | NIN_SELECT_ | NIN_KEYSELECT_ => show_menu(),
                    _ => {}
                }
                0
            }
            WM_UPDATE => {
                let b = Box::from_raw(lp as *mut (bool, update::CheckResult));
                let (manual, r) = *b;
                on_update_result(manual, r);
                0
            }
            WM_WIDGET_CLICK => {
                show_menu();
                0
            }
            WM_ENDSESSION => {
                if wp != 0 {
                    DestroyWindow(h);
                }
                0
            }
            WM_DEVICECHANGE => {
                if wp == DBT_DEVICEARRIVAL as usize || wp == DBT_DEVICEREMOVECOMPLETE as usize {
                    refresh_soon(2500);
                }
                1
            }
            WM_POWERBROADCAST => {
                if wp == PBT_APMRESUMEAUTOMATIC as usize {
                    refresh_soon(5000);
                }
                1
            }
            WM_TIMER => {
                if wp == TIMER_DEBOUNCE {
                    KillTimer(h, TIMER_DEBOUNCE);
                    SetEvent(a.refresh_evt);
                }
                if wp == TIMER_WIDGET {
                    reconcile();
                }
                if wp == TIMER_UPDATE {
                    SetTimer(h, TIMER_UPDATE, UPDATE_EVERY_MS, None);
                    if a.auto_update && update::is_installed() {
                        start_update_check(false);
                    }
                }
                if wp == TIMER_ALERT_ICON {
                    KillTimer(h, TIMER_ALERT_ICON);
                    if a.alert_icon_added {
                        tray_delete(ALERT_ICON_ID);
                        a.alert_icon_added = false;
                    }
                }
                0
            }
            WM_SETTINGCHANGE => {
                if lp != 0 && pwstr(lp as *const u16) == "ImmersiveColorSet" {
                    let light = read_light_taskbar();
                    if light != a.light_taskbar {
                        a.light_taskbar = light;
                        apply_menu_theme(light);
                        redraw_all();
                        widget::set_light_theme(light);
                    }
                }
                0
            }
            WM_DESTROY => {
                KillTimer(h, TIMER_WIDGET);
                KillTimer(h, TIMER_ALERT_ICON);
                if a.alert_icon_added {
                    tray_delete(ALERT_ICON_ID);
                    a.alert_icon_added = false;
                }
                widget::destroy();
                SetEvent(a.stop_evt);
                if let Some(t) = a.worker.take() {
                    let _ = t.join();
                }
                for e in a.entries.values() {
                    if e.added {
                        tray_delete(e.uid);
                    }
                    if !e.icon.is_null() {
                        DestroyIcon(e.icon);
                    }
                }
                if a.placeholder {
                    tray_delete(PLACEHOLDER_ID);
                }
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

const GUID_DEVINTERFACE_HID: GUID = GUID::from_u128(0x4D1E55B2_F16F_11CF_88CB_001111000030);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    unsafe {
        if args.iter().any(|a| a == "--quit") {
            // Für den Installer: laufende Instanz sauber beenden.
            let cls = w(WND_CLASS);
            let wnd = FindWindowW(cls.as_ptr(), std::ptr::null());
            if !wnd.is_null() {
                let mut pid = 0;
                GetWindowThreadProcessId(wnd, &mut pid);
                let running = if pid != 0 { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) } else { std::ptr::null_mut() };
                if running.is_null() {
                    std::process::exit(1);
                }
                PostMessageW(wnd, WM_CLOSE, 0, 0);
                let r = WaitForSingleObject(running, 30000);
                CloseHandle(running);
                std::process::exit(if r == WAIT_OBJECT_0 { 0 } else { 1 });
            }
            return;
        }
        if args.iter().any(|a| a == "--check-update") {
            // Test: prüfen und laden, aber nicht installieren.
            let out = match update::check_and_download() {
                update::CheckResult::UpToDate => format!("up to date ({})", update::current_version()),
                update::CheckResult::Ready(v, p) => format!("ready: {v} -> {}", p.display()),
                update::CheckResult::Failed(e) => format!("failed: {e}"),
            };
            let _ = std::fs::write(std::env::temp_dir().join("akku-monitor-update.txt"), out);
            return;
        }
        if args.iter().any(|a| a == "--list") {
            // Wie im Original: gefundene Geräte auflisten (hier in eine Datei, da ohne Konsole).
            let mut out = String::new();
            let list = hidapi::HidApi::new().map(|mut api| devices::poll_all(&mut api)).unwrap_or_default();
            if list.is_empty() {
                out += "No devices with battery info found.\n";
            }
            for s in &list {
                let state = if !s.connected { "off / not connected".to_string() } else if s.asleep { "asleep".into() } else if s.percent >= 0 { format!("{}%", s.percent) } else { "unknown".into() };
                out += &format!("{}  [{}]  {}{}{}   ({})\n", s.name, s.via, state,
                                if s.detail.is_empty() { String::new() } else { format!(" ({})", s.detail) },
                                format!("{}{}", if s.full { ", fully charged" } else if s.charging { ", charging" } else { "" },
                                        if s.docked { ", docked" } else { "" }), s.key);
            }
            for n in bluetooth::without_battery() {
                out += &format!("{n}  [Bluetooth]  connected, Windows has no battery level for it
");
            }
            let path = std::env::temp_dir().join("akku-monitor-list.txt");
            let _ = std::fs::write(&path, out);
            return;
        }
        let demo = args.iter().any(|a| a == "--demo");
        let mname = w(MUTEX_NAME);
        let mutex = CreateMutexW(std::ptr::null(), 1, mname.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let inst = GetModuleHandleW(std::ptr::null());
        let light = read_light_taskbar();
        APP = Some(App {
            inst,
            hwnd: std::ptr::null_mut(),
            refresh_evt: CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()),
            stop_evt: CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()),
            taskbar_created_msg: 0,
            light_taskbar: light,
            want_widget: true,
            use_widget: false,
            hide_last_known: false,
            show_disconnected: false,
            dim_sleeping: true,
            record_history: true,
            widget_monitor: "all".into(),
            widget_background: false,
            stacked: false,
            brand_colors: false,
            entries: BTreeMap::new(),
            alert_latches: BTreeMap::new(),
            alert_icon_added: false,
            placeholder: false,
            placeholder_icon: std::ptr::null_mut(),
            aliases: AliasMap::new(),
            hidden: BTreeSet::new(),
            order: Vec::new(),
            alert_overrides: BTreeMap::new(),
            demo,
            auto_update: true,
            mutex,
            update_busy: false,
            worker: None,
        });
        let a = app();
        apply_menu_theme(light);
        let tbc = w("TaskbarCreated");
        a.taskbar_created_msg = RegisterWindowMessageW(tbc.as_ptr());

        let cls = w(WND_CLASS);
        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(wnd_proc);
        wc.hInstance = inst;
        wc.lpszClassName = cls.as_ptr();
        RegisterClassExW(&wc);
        let title = w(APP_NAME);
        // Verstecktes Top-Level-Fenster (nicht nur Nachrichten), damit TaskbarCreated ankommt.
        a.hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, cls.as_ptr(), title.as_ptr(), WS_POPUP, 0, 0, 0, 0, std::ptr::null_mut(),
                                 std::ptr::null_mut(), inst, std::ptr::null());
        if a.hwnd.is_null() {
            std::process::exit(1);
        }
        let mut filter: DEV_BROADCAST_DEVICEINTERFACE_W = std::mem::zeroed();
        filter.dbcc_size = std::mem::size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32;
        filter.dbcc_devicetype = DBT_DEVTYP_DEVICEINTERFACE;
        filter.dbcc_classguid = GUID_DEVINTERFACE_HID;
        let notify = RegisterDeviceNotificationW(a.hwnd as HANDLE, &filter as *const _ as *const _, DEVICE_NOTIFY_WINDOW_HANDLE);

        a.want_widget = load_bool("TaskbarWidget", true);
        a.hide_last_known = load_bool("HideLastKnownDevices", false);
        a.show_disconnected = load_bool("ShowDisconnectedDevices", false);
        a.dim_sleeping = load_bool("DimSleepingDevices", true);
        a.record_history = load_bool("RecordBatteryHistory", true);
        a.widget_background = load_bool("WidgetBackground", false);
        a.brand_colors = load_bool("BrandColors", false);
        a.stacked = load_bool("StackedLayout", false);
        POLL_INTERVAL_MS.store(load_poll_seconds() * 1000, Ordering::Relaxed);
        widget::set_stacked(a.stacked);
        widget::set_background(a.widget_background);
        i18n::init();
        a.widget_monitor = load_widget_monitor();
        widget::set_monitor_choice(&a.widget_monitor.clone());
        load_hidden();
        load_order();
        load_aliases();
        load_alert_thresholds();
        if !demo {
            history::initialize();
        }
        reconcile();
        update_placeholder();
        SetTimer(a.hwnd, TIMER_WIDGET, 1000, None);
        a.auto_update = load_bool("AutoUpdate", true);
        SetTimer(a.hwnd, TIMER_UPDATE, UPDATE_FIRST_MS, None);
        let (hw, re, st) = (a.hwnd as isize, a.refresh_evt as isize, a.stop_evt as isize);
        a.worker = Some(std::thread::spawn(move || worker(hw, re, st, demo)));

        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        while PeekMessageW(&mut m, std::ptr::null_mut(), WM_RESULTS, WM_RESULTS, PM_REMOVE) != 0 {
            drop(Box::from_raw(m.lParam as *mut Vec<DeviceStatus>));
        }
        if !notify.is_null() {
            UnregisterDeviceNotification(notify);
        }
        let m = app().mutex;
        if !m.is_null() {
            ReleaseMutex(m);
        }
    }
}
