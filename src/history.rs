//! Akku-Verlauf: begrenzter lokaler Speicher, vorsichtige Restlaufzeit-Schätzung und das
//! Verlaufsfenster mit Diagramm. Port von battery_history.cpp, history_model.h und
//! runtime_estimate.h aus BatteryHub.

#![allow(static_mut_refs)]

use crate::i18n::{t, tf};
use crate::util::*;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const RETENTION: i64 = 30 * 24 * 3600;
const REGULAR_INTERVAL: i64 = 30 * 60;
const STATE_CHANGE_INTERVAL: i64 = 60;
const MAX_SAMPLES: usize = 16000;
const MAX_KEY_CHARS: usize = 256;
const SECONDS_PER_DAY: i64 = 86400;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const COMPACT_AFTER_BYTES: u64 = 1024 * 1024;
const MAX_CONNECTED_GAP: i64 = 4 * 3600;

#[derive(Clone, Debug)]
pub struct Sample {
    key: String,
    timestamp: i64,
    percent: i32,
    charging: bool,
}

fn valid(s: &Sample) -> bool {
    !s.key.is_empty() && s.key.chars().count() <= MAX_KEY_CHARS && !s.key.contains('\0') && (946684800..=4102444800).contains(&s.timestamp)
        && (0..=100).contains(&s.percent)
}

fn hex_key(key: &str) -> String {
    key.encode_utf16().map(|c| format!("{:08X}", c as u32)).collect()
}

fn unhex_key(enc: &str) -> Option<String> {
    if enc.is_empty() || enc.len() % 8 != 0 || enc.len() > MAX_KEY_CHARS * 8 {
        return None;
    }
    let mut v = Vec::new();
    for i in (0..enc.len()).step_by(8) {
        let n = u32::from_str_radix(enc.get(i..i + 8)?, 16).ok()?;
        if n == 0 || n > 0xFFFF {
            return None;
        }
        v.push(n as u16);
    }
    String::from_utf16(&v).ok()
}

fn parse_uint(s: &str) -> Option<i64> {
    if s.is_empty() || s.len() > 14 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn encode(s: &Sample) -> String {
    if !valid(s) {
        return String::new();
    }
    format!("{}\t{}\t{}\t{}\n", s.timestamp, hex_key(&s.key), s.percent, if s.charging { 1 } else { 0 })
}

fn decode(row: &str) -> Option<Sample> {
    let parts: Vec<&str> = row.split('\t').collect();
    if parts.len() != 4 {
        return None;
    }
    let ts = parse_uint(parts[0])?;
    let key = unhex_key(parts[1])?;
    let pct = parse_uint(parts[2])?;
    let chg = parse_uint(parts[3])?;
    if chg > 1 || pct > 100 {
        return None;
    }
    let s = Sample { key, timestamp: ts, percent: pct as i32, charging: chg != 0 };
    valid(&s).then_some(s)
}

#[derive(Default)]
struct Stats {
    count: usize,
    first: i32,
    last: i32,
    lowest: i32,
    highest: i32,
}

fn summarize(points: &[Sample]) -> Stats {
    let mut st = Stats { count: points.len(), first: -1, last: -1, lowest: -1, highest: -1 };
    if let (Some(f), Some(l)) = (points.first(), points.last()) {
        st.first = f.percent;
        st.last = l.percent;
        st.lowest = points.iter().map(|p| p.percent).min().unwrap();
        st.highest = points.iter().map(|p| p.percent).max().unwrap();
    }
    st
}

#[derive(Default, Clone)]
struct Store {
    samples: Vec<Sample>,
}

impl Store {
    fn prune(&mut self, now: i64) {
        self.samples.retain(|s| valid(s) && s.timestamp >= now - RETENTION && s.timestamp <= now + 3600);
        if self.samples.len() > MAX_SAMPLES {
            let drop = self.samples.len() - MAX_SAMPLES;
            self.samples.drain(..drop);
        }
    }
    fn record(&mut self, s: Sample) -> bool {
        if !valid(&s) {
            return false;
        }
        if let Some(prev) = self.samples.iter().rev().find(|p| p.key == s.key) {
            let elapsed = s.timestamp - prev.timestamp;
            if elapsed < STATE_CHANGE_INTERVAL {
                return false;
            }
            if elapsed < REGULAR_INTERVAL && s.charging == prev.charging {
                return false;
            }
        }
        let ts = s.timestamp;
        self.samples.push(s);
        self.prune(ts);
        true
    }
    fn load(&mut self, content: &str, now: i64) {
        self.samples.clear();
        let mut rest = content;
        while let Some(end) = rest.find('\n') {
            let mut row = &rest[..end];
            if row.ends_with('\r') {
                row = &row[..row.len() - 1];
            }
            if let Some(s) = decode(row) {
                self.samples.push(s);
            }
            if self.samples.len() > MAX_SAMPLES {
                self.samples.remove(0);
            }
            rest = &rest[end + 1..];
        }
        self.prune(now);
    }
    fn serialize(&self) -> String {
        self.samples.iter().map(encode).collect()
    }
    fn for_key(&self, key: &str, since: i64, now: i64) -> Vec<Sample> {
        let mut out: Vec<Sample> = self.samples.iter().filter(|s| s.key == key && s.timestamp >= since && s.timestamp <= now).cloned().collect();
        out.sort_by_key(|s| s.timestamp);
        out
    }
    fn clear(&mut self, key: &str) {
        self.samples.retain(|s| s.key != key);
    }
}

// ---------- Restlaufzeit (runtime_estimate.h) ----------

#[derive(PartialEq, Clone, Copy)]
enum RuntimeState {
    Ready,
    Charging,
    Stale,
    Insufficient,
    Flat,
}

struct RuntimeEstimate {
    state: RuntimeState,
    hours_remaining: f64,
    percent_per_hour: f64,
    sample_count: usize,
}

fn estimate_runtime(input: &[Sample], now: i64) -> RuntimeEstimate {
    let mut out = RuntimeEstimate { state: RuntimeState::Insufficient, hours_remaining: 0.0, percent_per_hour: 0.0, sample_count: 0 };
    if input.is_empty() {
        return out;
    }
    let mut points = input.to_vec();
    points.sort_by_key(|s| s.timestamp);
    let latest = points.last().unwrap().clone();
    if latest.timestamp > now + 300 || now - latest.timestamp > 3 * 3600 {
        out.state = RuntimeState::Stale;
        return out;
    }
    if latest.charging {
        out.state = RuntimeState::Charging;
        return out;
    }
    if latest.percent == 0 {
        out.state = RuntimeState::Ready;
        out.sample_count = 1;
        return out;
    }
    let mut start = points.len() - 1;
    while start > 0 {
        let (prev, cur) = (&points[start - 1], &points[start]);
        let gap = cur.timestamp - prev.timestamp;
        let diff = cur.percent - prev.percent;
        if prev.charging || cur.charging || gap <= 0 || gap > 4 * 3600 || diff > 1 || diff < -25 {
            break;
        }
        start -= 1;
    }
    out.sample_count = points.len() - start;
    if out.sample_count < 4 {
        return out;
    }
    let obs_hours = (latest.timestamp - points[start].timestamp) as f64 / 3600.0;
    if obs_hours < 2.0 {
        return out;
    }
    let drop = points[start].percent - latest.percent;
    if drop < 3 {
        out.state = RuntimeState::Flat;
        return out;
    }
    out.percent_per_hour = drop as f64 / obs_hours;
    if out.percent_per_hour < 0.02 || out.percent_per_hour > 12.0 {
        out.state = RuntimeState::Flat;
        return out;
    }
    out.hours_remaining = latest.percent as f64 / out.percent_per_hour;
    if !out.hours_remaining.is_finite() || out.hours_remaining > 90.0 * 24.0 {
        out.state = RuntimeState::Flat;
        return out;
    }
    out.state = RuntimeState::Ready;
    out
}

// ---------- Datei ----------

struct H {
    store: Store,
    file: Option<std::path::PathBuf>,
    view: HWND,
    btns: [HWND; 4],
    key: String,
    name: String,
    days: i64,
    light: bool,
    font: HFONT,
    title: HFONT,
    value: HFONT,
}

static mut HS: Option<H> = None;

fn h() -> &'static mut H {
    unsafe {
        let p = &mut *(&raw mut HS);
        p.get_or_insert_with(|| H {
            store: Store::default(),
            file: None,
            view: std::ptr::null_mut(),
            btns: [std::ptr::null_mut(); 4],
            key: String::new(),
            name: String::new(),
            days: 7,
            light: false,
            font: std::ptr::null_mut(),
            title: std::ptr::null_mut(),
            value: std::ptr::null_mut(),
        })
    }
}

const K24: i32 = 201;
const K7: i32 = 202;
const K30: i32 = 203;
const KCLEAR: i32 = 204;
const CLASS: &str = "AkkuMonitorHistoryViewer";

fn history_path() -> Option<std::path::PathBuf> {
    let dir = std::path::Path::new(&std::env::var_os("LOCALAPPDATA")?).join("AkkuMonitor");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("battery_history.tsv"))
}

fn rewrite() -> bool {
    let hs = h();
    let Some(file) = hs.file.clone() else { return false };
    let mut bytes = hs.store.serialize();
    while bytes.len() > 2 * 1024 * 1024 && !hs.store.samples.is_empty() {
        let drop = (hs.store.samples.len() / 8).max(1);
        hs.store.samples.drain(..drop);
        bytes = hs.store.serialize();
    }
    let tmp = file.with_extension("tsv.tmp");
    if std::fs::write(&tmp, bytes).is_err() || std::fs::rename(&tmp, &file).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}

fn append(s: &Sample) {
    use std::io::Write;
    let Some(file) = h().file.clone() else { return };
    let compact = match std::fs::OpenOptions::new().create(true).append(true).open(&file) {
        Ok(mut f) => f.write_all(encode(s).as_bytes()).is_ok() && f.metadata().map(|m| m.len() > COMPACT_AFTER_BYTES).unwrap_or(false),
        Err(_) => false,
    };
    if compact {
        rewrite();
    }
}

fn load() {
    let hs = h();
    let Some(file) = hs.file.clone() else { return };
    match std::fs::metadata(&file) {
        Ok(m) if m.len() > MAX_FILE_BYTES => {
            hs.file = None;
            return;
        }
        Err(_) => return,
        _ => {}
    }
    let Ok(data) = std::fs::read_to_string(&file) else { return };
    hs.store.load(&data, now_unix());
    if data != hs.store.serialize() {
        rewrite();
    }
}

pub fn initialize() {
    let hs = h();
    hs.store = Store::default();
    hs.file = history_path();
    load();
}

pub fn observe(key: &str, percent: i32, charging: bool) {
    let s = Sample { key: key.to_string(), timestamp: now_unix(), percent, charging };
    if h().store.record(s.clone()) {
        append(&s);
        let hs = h();
        if !hs.view.is_null() && key == hs.key {
            unsafe { InvalidateRect(hs.view, std::ptr::null(), 0) };
        }
    }
}

// ---------- Fenster ----------

struct Palette {
    bg: u32,
    panel: u32,
    chart: u32,
    border: u32,
    grid: u32,
    text: u32,
    muted: u32,
    blue: u32,
    green: u32,
    red: u32,
    btn_down: u32,
    btn_active: u32,
    btn_disabled_text: u32,
}

fn pal() -> Palette {
    if h().light {
        Palette { bg: rgb(243, 245, 248), panel: rgb(255, 255, 255), chart: rgb(250, 251, 253), border: rgb(209, 215, 224),
                  grid: rgb(228, 232, 238), text: rgb(24, 32, 44), muted: rgb(92, 104, 120), blue: rgb(0, 103, 192),
                  green: rgb(16, 137, 62), red: rgb(196, 43, 28), btn_down: rgb(214, 226, 240), btn_active: rgb(224, 236, 252),
                  btn_disabled_text: rgb(160, 168, 180) }
    } else {
        Palette { bg: rgb(12, 17, 28), panel: rgb(22, 30, 45), chart: rgb(15, 23, 37), border: rgb(47, 63, 83), grid: rgb(42, 55, 74),
                  text: rgb(232, 240, 252), muted: rgb(165, 182, 202), blue: rgb(91, 166, 255), green: rgb(59, 215, 158),
                  red: rgb(255, 132, 147), btn_down: rgb(38, 59, 83), btn_active: rgb(31, 67, 112), btn_disabled_text: rgb(102, 117, 136) }
    }
}

fn apps_use_light_theme() -> bool {
    reg_get_dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme").map(|v| v != 0).unwrap_or(true)
}

fn ui(wnd: HWND, v: i32) -> i32 {
    let d = if wnd.is_null() { 96 } else { (unsafe { GetDpiForWindow(wnd) }) as i32 };
    mul_div(v, if d != 0 { d } else { 96 }, 96)
}

fn apply_theme(wnd: HWND) {
    h().light = apps_use_light_theme();
    let dark: i32 = (!h().light) as i32;
    unsafe {
        let p = &dark as *const i32 as *const core::ffi::c_void;
        if windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(wnd, 20, p, 4) < 0 {
            windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(wnd, 19, p, 4);
        }
    }
}

fn create_ui_font(wnd: HWND, points: i32, weight: i32) -> HFONT {
    unsafe {
        let dc = GetDC(wnd);
        let d = if dc.is_null() { 96 } else { GetDeviceCaps(dc, LOGPIXELSY as i32) };
        if !dc.is_null() {
            ReleaseDC(wnd, dc);
        }
        let face = w("Segoe UI");
        CreateFontW(-mul_div(points, d, 72), 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_DEFAULT_PRECIS as u32,
                    CLIP_DEFAULT_PRECIS as u32, CLEARTYPE_QUALITY as u32, (DEFAULT_PITCH | FF_DONTCARE) as u32, face.as_ptr())
    }
}

fn layout_buttons(wnd: HWND) {
    let hs = h();
    let mut r = rect0();
    unsafe { GetClientRect(wnd, &mut r) };
    let (margin, y, height, gap, bw) = (ui(wnd, 24), ui(wnd, 18), ui(wnd, 38), ui(wnd, 10), ui(wnd, 150));
    unsafe {
        if !hs.btns[0].is_null() {
            MoveWindow(hs.btns[0], margin, y, bw, height, 1);
        }
        if !hs.btns[1].is_null() {
            MoveWindow(hs.btns[1], margin + bw + gap, y, bw, height, 1);
        }
        if !hs.btns[2].is_null() {
            MoveWindow(hs.btns[2], margin + 2 * (bw + gap), y, bw, height, 1);
        }
        if !hs.btns[3].is_null() {
            MoveWindow(hs.btns[3], r.right - margin - ui(wnd, 192), y, ui(wnd, 192), height, 1);
        }
    }
}

fn label(dc: HDC, s: &str, mut r: RECT, color: u32, flags: u32) {
    let ws: Vec<u16> = s.encode_utf16().collect();
    unsafe {
        SetTextColor(dc, color);
        SetBkMode(dc, TRANSPARENT as _);
        DrawTextW(dc, ws.as_ptr(), ws.len() as i32, &mut r, flags | DT_NOPREFIX);
    }
}

const DEF_FLAGS: u32 = DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS;

fn rounded_panel(dc: HDC, r: &RECT, fill: u32, outline: u32, radius: i32) {
    unsafe {
        let brush = CreateSolidBrush(fill);
        let pen = CreatePen(PS_SOLID, 1, outline);
        let ob = SelectObject(dc, brush as _);
        let op = SelectObject(dc, pen as _);
        RoundRect(dc, r.left, r.top, r.right, r.bottom, radius, radius);
        SelectObject(dc, op);
        SelectObject(dc, ob);
        DeleteObject(pen as _);
        DeleteObject(brush as _);
    }
}

fn font_or_default(f: HFONT) -> HGDIOBJ {
    if f.is_null() { unsafe { GetStockObject(DEFAULT_GUI_FONT) } } else { f as HGDIOBJ }
}

fn draw_button(item: &DRAWITEMSTRUCT) {
    if item.CtlType != ODT_BUTTON {
        return;
    }
    let hs = h();
    let p = pal();
    let disabled = item.itemState & ODS_DISABLED != 0;
    let down = item.itemState & ODS_SELECTED != 0;
    let focused = item.itemState & ODS_FOCUS != 0;
    let id = item.CtlID as i32;
    let active = (id == K24 && hs.days == 1) || (id == K7 && hs.days == 7) || (id == K30 && hs.days == 30);
    let clear = id == KCLEAR;
    let background = if disabled { p.bg } else if down { p.btn_down } else if active { p.btn_active } else { p.panel };
    let outline = if disabled { p.grid } else if active || focused { p.blue } else { p.border };
    let text_color = if disabled { p.btn_disabled_text } else if clear { p.red } else if active { p.text } else { p.muted };
    unsafe {
        let corners = CreateSolidBrush(p.bg);
        FillRect(item.hDC, &item.rcItem, corners);
        DeleteObject(corners as _);
        rounded_panel(item.hDC, &item.rcItem, background, outline, 12);
        let mut text = [0u16; 160];
        GetWindowTextW(item.hwndItem, text.as_mut_ptr(), 160);
        let old = SelectObject(item.hDC, font_or_default(hs.font));
        let mut r = item.rcItem;
        let inset = ui(item.hwndItem, 8);
        r.left += inset;
        r.right -= inset;
        label(item.hDC, &from_w(&text), r, text_color, DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);
        SelectObject(item.hDC, old);
    }
}

fn draw_stat(wnd: HWND, dc: HDC, r: &RECT, title: &str, value: &str, value_color: u32) {
    let p = pal();
    rounded_panel(dc, r, p.panel, p.border, ui(wnd, 12));
    let pad = ui(wnd, 14);
    label(dc, &t(title), RECT { left: r.left + pad, top: r.top + ui(wnd, 7), right: r.right - pad, bottom: r.top + ui(wnd, 34) }, p.muted, DEF_FLAGS);
    unsafe {
        let old = SelectObject(dc, font_or_default(h().value));
        label(dc, value, RECT { left: r.left + pad, top: r.top + ui(wnd, 35), right: r.right - pad, bottom: r.bottom - ui(wnd, 7) }, value_color, DEF_FLAGS);
        SelectObject(dc, old);
    }
}

fn format_remaining(hours: f64) -> String {
    let total = hours.round() as i64;
    let (days, hrs) = (total / 24, total % 24);
    if days > 0 {
        tf("~{days} d {hours} h", &[("days", days.to_string()), ("hours", hrs.to_string())])
    } else {
        tf("~{hours} h", &[("hours", total.to_string())])
    }
}

fn paint(wnd: HWND) {
    let hs = h();
    let p = pal();
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let screen = BeginPaint(wnd, &mut ps);
        let mut bounds = rect0();
        GetClientRect(wnd, &mut bounds);
        let (width, height) = (bounds.right, bounds.bottom);
        if width <= 0 || height <= 0 {
            EndPaint(wnd, &ps);
            return;
        }
        let buffer = CreateCompatibleDC(screen);
        let bitmap = if buffer.is_null() { std::ptr::null_mut() } else { CreateCompatibleBitmap(screen, width, height) };
        let old_bitmap = if !bitmap.is_null() { SelectObject(buffer, bitmap as _) } else { std::ptr::null_mut() };
        let dc = if !old_bitmap.is_null() { buffer } else { screen };
        let bg = CreateSolidBrush(p.bg);
        FillRect(dc, &bounds, bg);
        DeleteObject(bg as _);
        let old_font = SelectObject(dc, font_or_default(hs.font));
        SetBkMode(dc, TRANSPARENT as _);

        let margin = ui(wnd, 24);
        let now = now_unix();
        let points = hs.store.for_key(&hs.key, now - hs.days * SECONDS_PER_DAY, now);
        let stats = summarize(&points);
        let forecast = estimate_runtime(&hs.store.for_key(&hs.key, now - RETENTION, now), now);

        let saved = SelectObject(dc, font_or_default(hs.title));
        label(dc, &format!("{}  •  {}", t("Battery history"), hs.name), RECT { left: margin, top: ui(wnd, 73), right: width - margin, bottom: ui(wnd, 113) }, p.text, DEF_FLAGS);
        SelectObject(dc, saved);

        let gap = ui(wnd, 10);
        let card_w = (width - 2 * margin - 4 * gap) / 5;
        let mut delta = "—".to_string();
        let mut delta_color = p.text;
        if stats.count >= 2 {
            let change = stats.last - stats.first;
            delta = format!("{}{} {}", if change > 0 { "+" } else { "" }, change, t("pp"));
            delta_color = if change < 0 { p.red } else if change > 0 { p.green } else { p.text };
        }
        let dash = |v: i32| if stats.count > 0 { format!("{v}%") } else { "—".into() };
        let values = [dash(stats.last), dash(stats.lowest), dash(stats.highest), delta, stats.count.to_string()];
        let headings = ["Latest", "Lowest", "Highest", "Change", "Readings"];
        for i in 0..5 {
            let x = margin + i as i32 * (card_w + gap);
            let card = RECT { left: x, top: ui(wnd, 148), right: x + card_w, bottom: ui(wnd, 224) };
            let c = if i == 3 { delta_color } else if i == 0 { p.blue } else { p.text };
            draw_stat(wnd, dc, &card, headings[i], &values[i], c);
        }

        let eta = RECT { left: margin, top: ui(wnd, 237), right: width - margin, bottom: ui(wnd, 318) };
        rounded_panel(dc, &eta, p.panel, p.border, ui(wnd, 12));
        label(dc, &t("Estimated time to empty"), RECT { left: eta.left + ui(wnd, 16), top: eta.top + ui(wnd, 7), right: eta.right - ui(wnd, 20), bottom: eta.top + ui(wnd, 34) }, p.muted, DEF_FLAGS);
        let mut eta_color = p.blue;
        let (eta_value, eta_details) = match forecast.state {
            RuntimeState::Ready => {
                let rate = (forecast.percent_per_hour * 240.0).round() as i64;
                (format_remaining(forecast.hours_remaining),
                 tf("Recent discharge: {rate} pp/day · {count} readings", &[("rate", format!("{}.{}", rate / 10, rate % 10)), ("count", forecast.sample_count.to_string())]))
            }
            RuntimeState::Charging => {
                eta_color = p.green;
                (t("Currently charging"), t("No remaining-time estimate while charging."))
            }
            RuntimeState::Stale => (t("No recent readings"), t("Connect the device to resume forecasting.")),
            RuntimeState::Flat => (t("Insufficient decline"), t("A measurable discharge trend is required.")),
            RuntimeState::Insufficient => (t("Collecting data"), t("Requires 4 readings, 2 hours and 3 percentage points of decline.")),
        };
        let saved = SelectObject(dc, font_or_default(hs.value));
        label(dc, &eta_value, RECT { left: eta.left + ui(wnd, 16), top: eta.top + ui(wnd, 34), right: eta.left + (eta.right - eta.left) / 2 - ui(wnd, 8), bottom: eta.bottom - ui(wnd, 7) }, eta_color, DEF_FLAGS);
        SelectObject(dc, saved);
        label(dc, &eta_details, RECT { left: eta.left + (eta.right - eta.left) / 2, top: eta.top + ui(wnd, 43), right: eta.right - ui(wnd, 16), bottom: eta.bottom - ui(wnd, 5) }, p.muted, DEF_FLAGS);

        let panel = RECT { left: margin, top: ui(wnd, 332), right: width - margin, bottom: height - ui(wnd, 22) };
        if panel.bottom > panel.top + ui(wnd, 140) {
            rounded_panel(dc, &panel, p.panel, p.border, ui(wnd, 14));
            label(dc, &t("Blue: recorded battery; green: measured increase"), RECT { left: panel.left + ui(wnd, 18), top: panel.top + ui(wnd, 8), right: panel.right - ui(wnd, 12), bottom: panel.top + ui(wnd, 42) }, p.muted, DEF_FLAGS);
            let plot = RECT { left: panel.left + ui(wnd, 78), top: panel.top + ui(wnd, 53), right: panel.right - ui(wnd, 21), bottom: panel.bottom - ui(wnd, 61) };
            if plot.right - plot.left > ui(wnd, 120) && plot.bottom - plot.top > ui(wnd, 68) {
                rounded_panel(dc, &plot, p.chart, p.chart, ui(wnd, 6));
                let (gw, gh) = (plot.right - plot.left, plot.bottom - plot.top);
                let grid = CreatePen(PS_SOLID, 1, p.grid);
                let old_pen = SelectObject(dc, grid as _);
                for i in 0..=4 {
                    let y = plot.top + gh * i / 4;
                    MoveToEx(dc, plot.left, y, std::ptr::null_mut());
                    LineTo(dc, plot.right, y);
                    label(dc, &format!("{}%", 100 - i * 25), RECT { left: panel.left + ui(wnd, 9), top: y - ui(wnd, 13), right: plot.left - ui(wnd, 10), bottom: y + ui(wnd, 13) }, p.muted, DT_RIGHT | DT_SINGLELINE | DT_VCENTER);
                    let x = plot.left + gw * i / 4;
                    MoveToEx(dc, x, plot.top, std::ptr::null_mut());
                    LineTo(dc, x, plot.bottom);
                }
                SelectObject(dc, old_pen);
                DeleteObject(grid as _);

                if !points.is_empty() {
                    let data_pen = CreatePen(PS_SOLID, ui(wnd, 2), p.blue);
                    let inc_pen = CreatePen(PS_SOLID, ui(wnd, 2), p.green);
                    let since = now - hs.days * SECONDS_PER_DAY;
                    let span = hs.days * SECONDS_PER_DAY;
                    let (mut px_, mut py_, mut plevel, mut ptime) = (0, 0, -1, -1i64);
                    for (i, smp) in points.iter().enumerate() {
                        let mut x = plot.left + (gw as i64 * 0.max(smp.timestamp - since) / span) as i32;
                        let y = plot.bottom - gh * smp.percent / 100;
                        x = x.clamp(plot.left, plot.right);
                        let joined = ptime >= 0 && smp.timestamp > ptime && smp.timestamp - ptime <= MAX_CONNECTED_GAP && (smp.percent - plevel).abs() <= 25;
                        let rising = joined && smp.percent > plevel;
                        if joined {
                            let op = SelectObject(dc, if rising { inc_pen } else { data_pen } as _);
                            MoveToEx(dc, px_, py_, std::ptr::null_mut());
                            LineTo(dc, x, y);
                            SelectObject(dc, op);
                        }
                        if i + 1 == points.len() || points.len() <= 70 || i % 10 == 0 {
                            let dot = CreateSolidBrush(if rising { p.green } else { p.blue });
                            let op = SelectObject(dc, GetStockObject(NULL_PEN));
                            let ob = SelectObject(dc, dot as _);
                            let rad = ui(wnd, if i + 1 == points.len() { 4 } else { 2 });
                            Ellipse(dc, x - rad, y - rad, x + rad + 1, y + rad + 1);
                            SelectObject(dc, ob);
                            SelectObject(dc, op);
                            DeleteObject(dot as _);
                        }
                        px_ = x;
                        py_ = y;
                        ptime = smp.timestamp;
                        plevel = smp.percent;
                    }
                    DeleteObject(data_pen as _);
                    DeleteObject(inc_pen as _);
                } else {
                    label(dc, &t("No readings in this period"), RECT { left: plot.left + ui(wnd, 10), top: plot.top, right: plot.right - ui(wnd, 10), bottom: plot.bottom }, p.muted, DT_CENTER | DT_SINGLELINE | DT_VCENTER);
                }
                let start = if hs.days == 1 { t("24h ago") } else { tf("{days} days ago", &[("days", hs.days.to_string())]) };
                label(dc, &start, RECT { left: plot.left, top: plot.bottom + ui(wnd, 5), right: plot.left + ui(wnd, 160), bottom: plot.bottom + ui(wnd, 31) }, p.muted, DEF_FLAGS);
                label(dc, &t("Now"), RECT { left: plot.right - ui(wnd, 65), top: plot.bottom + ui(wnd, 5), right: plot.right, bottom: plot.bottom + ui(wnd, 31) }, p.muted, DT_RIGHT | DT_SINGLELINE | DT_VCENTER);
                label(dc, &t("Trend estimates are approximate; missing samples are not inferred."), RECT { left: plot.left, top: plot.bottom + ui(wnd, 31), right: plot.right, bottom: plot.bottom + ui(wnd, 57) }, p.muted, DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);
            }
        }
        SelectObject(dc, old_font);
        if dc == buffer && !old_bitmap.is_null() {
            BitBlt(screen, 0, 0, width, height, dc, 0, 0, SRCCOPY);
            SelectObject(buffer, old_bitmap);
        }
        if !bitmap.is_null() {
            DeleteObject(bitmap as _);
        }
        if !buffer.is_null() {
            DeleteDC(buffer);
        }
        EndPaint(wnd, &ps);
    }
}

fn message_box(wnd: HWND, text: &str, caption: &str, flags: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    let (tw, cw) = (w(text), w(caption));
    unsafe { MessageBoxW(wnd, tw.as_ptr(), cw.as_ptr(), flags) }
}

unsafe extern "system" fn view_proc(wnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let hs = h();
    match msg {
        WM_SETTINGCHANGE => {
            if lp != 0 && pwstr(lp as *const u16) == "ImmersiveColorSet" {
                apply_theme(wnd);
                unsafe {
                    InvalidateRect(wnd, std::ptr::null(), 1);
                    for b in hs.btns {
                        if !b.is_null() {
                            InvalidateRect(b, std::ptr::null(), 1);
                        }
                    }
                }
            }
        }
        WM_CREATE => {
            apply_theme(wnd);
            hs.font = create_ui_font(wnd, 10, FW_NORMAL as i32);
            hs.title = create_ui_font(wnd, 17, FW_SEMIBOLD as i32);
            hs.value = create_ui_font(wnd, 16, FW_SEMIBOLD as i32);
            let inst = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) };
            let btn = w("BUTTON");
            let mk = |text: &str, id: i32| {
                let tw = w(&t(text));
                unsafe {
                    CreateWindowExW(0, btn.as_ptr(), tw.as_ptr(), WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32, 0, 0, 110, 28, wnd,
                                    id as isize as HMENU, inst, std::ptr::null())
                }
            };
            hs.btns = [mk("Last 24 hours", K24), mk("Last 7 days", K7), mk("Last 30 days", K30), mk("Clear device history", KCLEAR)];
            for b in hs.btns {
                unsafe { SendMessageW(b, WM_SETFONT, font_or_default(hs.font) as usize, 1) };
            }
            layout_buttons(wnd);
            return 0;
        }
        WM_GETMINMAXINFO => {
            if lp != 0 {
                let info = unsafe { &mut *(lp as *mut MINMAXINFO) };
                info.ptMinTrackSize.x = ui(wnd, 880);
                info.ptMinTrackSize.y = ui(wnd, 620);
            }
            return 0;
        }
        WM_SIZE => {
            layout_buttons(wnd);
            unsafe { InvalidateRect(wnd, std::ptr::null(), 1) };
            return 0;
        }
        WM_DRAWITEM => {
            if lp != 0 {
                let item = unsafe { &*(lp as *const DRAWITEMSTRUCT) };
                if item.CtlType == ODT_BUTTON {
                    draw_button(item);
                    return 1;
                }
            }
        }
        WM_COMMAND => {
            match loword(wp) as i32 {
                K24 => hs.days = 1,
                K7 => hs.days = 7,
                K30 => hs.days = 30,
                KCLEAR => {
                    if message_box(wnd, &t("Clear battery history for this device?"), &t("Battery history"), MB_YESNO | MB_ICONQUESTION) == IDYES {
                        let before = hs.store.clone();
                        let key = hs.key.clone();
                        hs.store.clear(&key);
                        if !rewrite() {
                            h().store = before;
                            message_box(wnd, &t("Could not save history changes."), &t("Battery history"), MB_OK | MB_ICONWARNING);
                        }
                    }
                }
                _ => return 0,
            }
            unsafe { InvalidateRect(wnd, std::ptr::null(), 1) };
            return 0;
        }
        WM_PAINT => {
            paint(wnd);
            return 0;
        }
        WM_ERASEBKGND => return 1,
        WM_CLOSE => {
            unsafe { DestroyWindow(wnd) };
            return 0;
        }
        WM_DESTROY => {
            hs.view = std::ptr::null_mut();
            hs.btns = [std::ptr::null_mut(); 4];
            unsafe {
                for f in [hs.font, hs.title, hs.value] {
                    if !f.is_null() {
                        DeleteObject(f as _);
                    }
                }
            }
            hs.font = std::ptr::null_mut();
            hs.title = std::ptr::null_mut();
            hs.value = std::ptr::null_mut();
            return 0;
        }
        _ => {}
    }
    unsafe { DefWindowProcW(wnd, msg, wp, lp) }
}

pub fn show(key: &str, display_name: &str) {
    if key.is_empty() {
        return;
    }
    let hs = h();
    hs.key = key.to_string();
    hs.name = display_name.to_string();
    let title = w(&format!("{} - {}", t("Battery history"), display_name));
    unsafe {
        if !hs.view.is_null() && IsWindow(hs.view) != 0 {
            SetWindowTextW(hs.view, title.as_ptr());
            ShowWindow(hs.view, SW_RESTORE);
            SetForegroundWindow(hs.view);
            InvalidateRect(hs.view, std::ptr::null(), 1);
            return;
        }
        let inst = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
        let cls = w(CLASS);
        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc.hInstance = inst;
        wc.lpfnWndProc = Some(view_proc);
        wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
        wc.hIcon = LoadIconW(inst, 1 as _);
        wc.lpszClassName = cls.as_ptr();
        RegisterClassExW(&wc);
        hs.days = 7;
        let mut b = RECT { left: 0, top: 0, right: 1020, bottom: 710 };
        AdjustWindowRectEx(&mut b, WS_OVERLAPPEDWINDOW, 0, WS_EX_APPWINDOW);
        hs.view = CreateWindowExW(WS_EX_APPWINDOW, cls.as_ptr(), title.as_ptr(), WS_OVERLAPPEDWINDOW, CW_USEDEFAULT, CW_USEDEFAULT,
                                  b.right - b.left, b.bottom - b.top, std::ptr::null_mut(), std::ptr::null_mut(), inst, std::ptr::null());
        if hs.view.is_null() {
            return;
        }
        ShowWindow(hs.view, SW_SHOWNORMAL);
        UpdateWindow(hs.view);
        SetForegroundWindow(hs.view);
    }
}
