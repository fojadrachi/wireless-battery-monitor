//! Schmaler transparenter Streifen in der Taskleiste links vom Infobereich:
//! "[Symbol] 30%  [Symbol] 93%". Ein geschichtetes Kindfenster der Taskleiste (Shell_TrayWnd),
//! mit je einer Kopie auf den Taskleisten weiterer Monitore (Shell_SecondaryTrayWnd).
//! Port von widget.cpp aus BatteryHub.

#![allow(static_mut_refs)]

use crate::brands::{accent_color, Accent};
use crate::i18n::{t, tf};
use crate::model::DeviceKind;
use crate::util::*;
use std::sync::Mutex;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Default)]
pub struct WidgetItem {
    pub kind: DeviceKind,
    pub percent: i32,
    pub text: String,
    pub charging: bool,
    pub full: bool,
    pub dim: bool,
    pub low: bool,
    pub bluetooth: bool,
    pub label: String,
    pub accent: Accent,
}

pub struct MonitorOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Copy)]
struct Mirror {
    bar: HWND,
    wnd: HWND,
    hover: bool,
    size: SIZE,
    dpi: i32,
    last_left: i32,
}

struct G {
    wnd: HWND,
    taskbar: HWND,
    tip: HWND,
    notify: HWND,
    rebar: HWND,
    click_msg: u32,
    light: bool,
    hover: bool,
    background: bool,
    stacked: bool,
    vertical: bool,
    items: Vec<WidgetItem>,
    tip_text: Vec<u16>,
    last_tray: RECT,
    last_bar: RECT,
    last_widgets: RECT,
    last_w: i32,
    last_dpi: i32,
    last_left: i32,
    size: SIZE,
    inst: HINSTANCE,
    mirrors: Vec<Mirror>,
    choice: String,
    primary_hidden: bool,
    dpi_bar: HWND,
    registered: bool,
}

static mut S: G = G {
    wnd: std::ptr::null_mut(),
    taskbar: std::ptr::null_mut(),
    tip: std::ptr::null_mut(),
    notify: std::ptr::null_mut(),
    rebar: std::ptr::null_mut(),
    click_msg: 0,
    light: false,
    hover: false,
    background: false,
    stacked: false,
    vertical: false,
    items: Vec::new(),
    tip_text: Vec::new(),
    last_tray: rect0(),
    last_bar: rect0(),
    last_widgets: rect0(),
    last_w: -1,
    last_dpi: 0,
    last_left: -1,
    size: SIZE { cx: 0, cy: 0 },
    inst: std::ptr::null_mut(),
    mirrors: Vec::new(),
    choice: String::new(),
    primary_hidden: false,
    dpi_bar: std::ptr::null_mut(),
    registered: false,
};

fn g() -> &'static mut G {
    // Nur vom UI-Thread benutzt (wie die globalen Variablen im Original).
    unsafe { &mut *(&raw mut S) }
}

const CLASS: &str = "AkkuMonitorWidget";
const NOT_PLACED: i32 = i32::MIN;

fn dpi() -> i32 {
    let s = g();
    let bar = if !s.dpi_bar.is_null() { s.dpi_bar } else { s.taskbar };
    let d = if bar.is_null() { 0 } else { unsafe { GetDpiForWindow(bar) } };
    if d != 0 { d as i32 } else { 96 }
}

fn px(v: i32) -> i32 {
    mul_div(v, dpi(), 96)
}

// ---------- Widgets-Knopf und Zweit-Taskleisten per UI Automation (eigener Thread) ----------

#[derive(Clone, Copy, Default)]
struct Span {
    bar: isize,
    measured: bool,
    has_clock: bool,
    clock_left: i32,
    has_widgets: bool,
    widgets: (i32, i32, i32, i32),
    has_tasks: bool,
    tasks_right: i32,
}

#[derive(Default)]
struct Wb {
    rect: (i32, i32, i32, i32),
    rect_bar: isize,
    taskbar: isize,
    spans: Vec<Span>,
    mirrors: bool,
    wake: isize,
}

static WB: Mutex<Wb> = Mutex::new(Wb { rect: (0, 0, 0, 0), rect_bar: 0, taskbar: 0, spans: Vec::new(), mirrors: false, wake: 0 });

fn is_xaml_taskbar(bar: HWND) -> bool {
    let cls = w("Windows.UI.Composition.DesktopWindowContentBridge");
    unsafe { !FindWindowExW(bar, std::ptr::null_mut(), cls.as_ptr(), std::ptr::null()).is_null() }
}

mod uia {
    use super::Span;
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Accessibility::*;

    pub struct Uia(IUIAutomation);

    pub fn create() -> Option<Uia> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok().map(Uia)
        }
    }

    fn cond(u: &Uia, prop: UIA_PROPERTY_ID, value: &str) -> Option<IUIAutomationCondition> {
        unsafe { u.0.CreatePropertyCondition(prop, &VARIANT::from(value)).ok() }
    }

    fn rect_of(el: &IUIAutomationElement) -> Option<(i32, i32, i32, i32)> {
        unsafe { el.CurrentBoundingRectangle().ok().map(|r: RECT| (r.left, r.top, r.right, r.bottom)) }
    }

    pub fn widgets_button(u: &Uia, bar: isize) -> Option<IUIAutomationElement> {
        unsafe {
            let root = u.0.ElementFromHandle(HWND(bar as *mut _)).ok()?;
            let c = cond(u, UIA_AutomationIdPropertyId, "WidgetsButton")?;
            root.FindFirst(TreeScope_Descendants, &c).ok()
        }
    }

    pub fn element_rect(el: &IUIAutomationElement) -> Option<(i32, i32, i32, i32)> {
        rect_of(el)
    }

    pub fn measure_span(u: &Uia, bar: isize, br: (i32, i32, i32, i32)) -> Span {
        let mut s = Span { bar, ..Default::default() };
        let Ok(root) = (unsafe { u.0.ElementFromHandle(HWND(bar as *mut _)) }) else { return s };
        s.measured = true;
        let in_bar = |r: (i32, i32, i32, i32)| r.2 > r.0 && r.0 >= br.0 && r.2 <= br.2;
        unsafe {
            if let Some(c) = cond(u, UIA_AutomationIdPropertyId, "SystemTrayIcon") {
                if let Ok(all) = root.FindAll(TreeScope_Descendants, &c) {
                    for i in 0..all.Length().unwrap_or(0) {
                        if let Some(r) = all.GetElement(i).ok().and_then(|e| rect_of(&e)) {
                            if in_bar(r) && (!s.has_clock || r.0 < s.clock_left) {
                                s.clock_left = r.0;
                                s.has_clock = true;
                            }
                        }
                    }
                }
            }
            if let Some(c) = cond(u, UIA_AutomationIdPropertyId, "WidgetsButton") {
                if let Some(r) = root.FindFirst(TreeScope_Descendants, &c).ok().and_then(|e| rect_of(&e)) {
                    if in_bar(r) {
                        s.widgets = r;
                        s.has_widgets = true;
                    }
                }
            }
            if let Some(c) = cond(u, UIA_ClassNamePropertyId, "Taskbar.TaskListButtonAutomationPeer") {
                if let Ok(all) = root.FindAll(TreeScope_Descendants, &c) {
                    for i in 0..all.Length().unwrap_or(0) {
                        if let Some(r) = all.GetElement(i).ok().and_then(|e| rect_of(&e)) {
                            if in_bar(r) && (!s.has_clock || r.2 <= s.clock_left) && (!s.has_widgets || r.2 <= s.widgets.0)
                                && (!s.has_tasks || r.2 > s.tasks_right)
                            {
                                s.tasks_right = r.2;
                                s.has_tasks = true;
                            }
                        }
                    }
                }
            }
        }
        s
    }
}

fn widgets_button_thread(wake: isize) {
    use windows_sys::Win32::System::Threading::{WaitForSingleObject, INFINITE};
    let mut u: Option<uia::Uia> = None;
    let mut btn = None;
    let mut btn_bar: isize = 0;
    loop {
        unsafe { WaitForSingleObject(wake as HANDLE, if btn_bar != 0 { 5000 } else { INFINITE }) };
        let bar = WB.lock().map(|w| w.taskbar).unwrap_or(0);
        if bar != btn_bar {
            btn = None;
        }
        btn_bar = bar;
        let mut r = (0, 0, 0, 0);
        if bar != 0 && unsafe { IsWindow(bar as HWND) } != 0 {
            if u.is_none() {
                u = uia::create();
            }
            if let Some(uu) = &u {
                if btn.is_none() {
                    btn = uia::widgets_button(uu, bar);
                }
                match btn.as_ref().and_then(uia::element_rect) {
                    Some(rr) => r = rr,
                    None => btn = None,
                }
            }
        }
        let mirrors = WB.lock().map(|w| w.mirrors).unwrap_or(false);
        let mut spans = Vec::new();
        if bar != 0 && mirrors {
            if let Some(uu) = &u {
                let cls = w("Shell_SecondaryTrayWnd");
                let mut t = unsafe { FindWindowExW(std::ptr::null_mut(), std::ptr::null_mut(), cls.as_ptr(), std::ptr::null()) };
                while !t.is_null() {
                    if is_xaml_taskbar(t) {
                        let mut br = rect0();
                        unsafe { GetWindowRect(t, &mut br) };
                        spans.push(uia::measure_span(uu, t as isize, (br.left, br.top, br.right, br.bottom)));
                    }
                    t = unsafe { FindWindowExW(std::ptr::null_mut(), t, cls.as_ptr(), std::ptr::null()) };
                }
            }
        }
        if let Ok(mut w) = WB.lock() {
            if bar != 0 && w.taskbar == bar {
                w.rect = r;
                w.rect_bar = bar;
            }
            w.spans = spans;
        }
    }
}

fn watch_widgets_button(taskbar: HWND) {
    use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent};
    let wake = {
        let mut w = WB.lock().unwrap();
        w.taskbar = taskbar as isize;
        if w.wake == 0 {
            let e = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
            if !e.is_null() {
                w.wake = e as isize;
                let we = e as isize;
                std::thread::spawn(move || widgets_button_thread(we));
            }
        }
        w.wake
    };
    if wake != 0 {
        unsafe { SetEvent(wake as HANDLE) };
    }
}

fn widgets_button_rect() -> RECT {
    let w = WB.lock().unwrap();
    if w.rect_bar != 0 && w.rect_bar == w.taskbar {
        RECT { left: w.rect.0, top: w.rect.1, right: w.rect.2, bottom: w.rect.3 }
    } else {
        rect0()
    }
}

fn set_measure_mirrors(on: bool) {
    use windows_sys::Win32::System::Threading::SetEvent;
    let (changed, wake) = {
        let mut w = WB.lock().unwrap();
        let changed = w.mirrors != on;
        w.mirrors = on;
        (changed, w.wake)
    };
    if changed && on && wake != 0 {
        unsafe { SetEvent(wake as HANDLE) };
    }
}

// ---------- Zeichnen ----------

fn apply_tip_theme() {
    let s = g();
    if s.tip.is_null() {
        return;
    }
    let theme = w(if s.light { "Explorer" } else { "DarkMode_Explorer" });
    unsafe {
        windows_sys::Win32::UI::Controls::SetWindowTheme(s.tip, theme.as_ptr(), std::ptr::null());
        SendMessageW(s.tip, WM_THEMECHANGED, 0, 0);
    }
}

fn glyph(k: DeviceKind, bluetooth: bool) -> u16 {
    match k {
        DeviceKind::Headset => 0xE7F6,
        DeviceKind::Mouse => 0xE962,
        DeviceKind::Keyboard => 0xE765,
        DeviceKind::Gamepad => 0xE7FC,
        DeviceKind::Phone => 0xE8EA,
        DeviceKind::Microphone => 0xE720,
        DeviceKind::Other => if bluetooth { 0xE702 } else { 0xE772 },
    }
}
const BOLT: u16 = 0xE945;
const CHECK: u16 = 0xE73E;
const BATTERY_UNKNOWN: u16 = 0xE996;

/// Schrift erzeugen; fehlt die gewünschte, die Ersatzschrift nehmen.
fn make_font(px_h: i32, weight: i32, face: &str, fallback: &str, fallback_px: i32) -> HFONT {
    for f in [face, fallback] {
        let h_px = if f == fallback && fallback_px > 0 { fallback_px } else { px_h };
        let fw = w(f);
        unsafe {
            let h = CreateFontW(-h_px, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_DEFAULT_PRECIS as u32,
                                CLIP_DEFAULT_PRECIS as u32, CLEARTYPE_QUALITY as u32, DEFAULT_PITCH as u32, fw.as_ptr());
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            let o = SelectObject(dc, h as _);
            let mut got = [0u16; 32];
            GetTextFaceW(dc, 32, got.as_mut_ptr());
            SelectObject(dc, o);
            DeleteDC(dc);
            if from_w(&got).eq_ignore_ascii_case(f) || f == fallback {
                return h;
            }
            DeleteObject(h as _);
        }
    }
    std::ptr::null_mut()
}

#[derive(Clone, Default)]
struct Mask {
    w: i32,
    h: i32,
    a: Vec<u8>,
}

fn render_mask(s: &[u16], font: HFONT, dark_on_light: bool) -> Mask {
    let mut m = Mask::default();
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        let of = SelectObject(dc, font as _);
        let mut sz = SIZE { cx: 0, cy: 0 };
        GetTextExtentPoint32W(dc, s.as_ptr(), s.len() as i32, &mut sz);
        let (ww, hh) = (sz.cx + 8, sz.cy + 8);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = ww;
        bi.bmiHeader.biHeight = -hh;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if bmp.is_null() || bits.is_null() {
            SelectObject(dc, of);
            DeleteDC(dc);
            return m;
        }
        let ob = SelectObject(dc, bmp as _);
        let p = std::slice::from_raw_parts_mut(bits as *mut u32, (ww * hh) as usize);
        p.fill(if dark_on_light { 0xFFFFFFFF } else { 0 });
        SetBkMode(dc, TRANSPARENT as _);
        SetTextColor(dc, if dark_on_light { 0 } else { 0x00FFFFFF });
        TextOutW(dc, 4, 4, s.as_ptr(), s.len() as i32);
        GdiFlush();
        let cover = |v: u32| -> u8 {
            let c = ((v & 0xFF) + ((v >> 8) & 0xFF) + ((v >> 16) & 0xFF)) / 3;
            (if dark_on_light { 255 - c } else { c }) as u8
        };
        let (mut x0, mut y0, mut x1, mut y1) = (ww, hh, -1, -1);
        for y in 0..hh {
            for x in 0..ww {
                if cover(p[(y * ww + x) as usize]) != 0 {
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
            }
        }
        if x1 >= 0 {
            m.w = x1 - x0 + 1;
            m.h = y1 - y0 + 1;
            m.a = vec![0; (m.w * m.h) as usize];
            for y in 0..m.h {
                for x in 0..m.w {
                    m.a[(y * m.w + x) as usize] = cover(p[((y0 + y) * ww + x0 + x) as usize]);
                }
            }
        }
        SelectObject(dc, ob);
        SelectObject(dc, of);
        DeleteObject(bmp as _);
        DeleteDC(dc);
    }
    m
}

#[derive(Clone, Copy, Default)]
struct Rgb {
    r: i32,
    g: i32,
    b: i32,
}
const fn rgbc(r: i32, g: i32, b: i32) -> Rgb {
    Rgb { r, g, b }
}

struct Canvas {
    w: i32,
    h: i32,
    px: Vec<u32>, // vormultipliziertes BGRA
}

impl Canvas {
    fn new(w: i32, h: i32) -> Self {
        Canvas { w, h, px: vec![0; (w * h) as usize] }
    }
    fn blend(&mut self, x: i32, y: i32, c: Rgb, a: i32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h || a <= 0 {
            return;
        }
        let i = (y * self.w + x) as usize;
        let d = self.px[i];
        let inv = 255 - a;
        let (da, dr, dg, db) = ((d >> 24) as i32, ((d >> 16) & 255) as i32, ((d >> 8) & 255) as i32, (d & 255) as i32);
        let oa = a + da * inv / 255;
        let or = c.r * a / 255 + dr * inv / 255;
        let og = c.g * a / 255 + dg * inv / 255;
        let ob = c.b * a / 255 + db * inv / 255;
        self.px[i] = (oa as u32) << 24 | (or as u32) << 16 | (og as u32) << 8 | ob as u32;
    }
    fn draw(&mut self, m: &Mask, x: i32, y: i32, c: Rgb, alpha: i32) {
        for j in 0..m.h {
            for i in 0..m.w {
                self.blend(x + i, y + j, c, m.a[(j * m.w + i) as usize] as i32 * alpha / 255);
            }
        }
    }
}

/// Weicher Schein um ein Symbol (Markenfarben).
fn halo(m: &Mask, r: i32) -> Mask {
    let gw = m.w + 2 * r;
    let gh = m.h + 2 * r;
    let mut a = vec![0i32; (gw * gh) as usize];
    let mut tmp = vec![0i32; a.len()];
    for y in 0..m.h {
        for x in 0..m.w {
            a[((y + r) * gw + x + r) as usize] = m.a[(y * m.w + x) as usize] as i32;
        }
    }
    let n = 2 * r + 1;
    for _ in 0..2 {
        for y in 0..gh {
            for x in 0..gw {
                let mut s = 0;
                for k in -r..=r {
                    if x + k >= 0 && x + k < gw {
                        s += a[(y * gw + x + k) as usize];
                    }
                }
                tmp[(y * gw + x) as usize] = s / n;
            }
        }
        for y in 0..gh {
            for x in 0..gw {
                let mut s = 0;
                for k in -r..=r {
                    if y + k >= 0 && y + k < gh {
                        s += tmp[((y + k) * gw + x) as usize];
                    }
                }
                a[(y * gw + x) as usize] = s / n;
            }
        }
    }
    Mask { w: gw, h: gh, a: a.iter().map(|&v| (v * 3).min(255) as u8).collect() }
}

#[derive(Clone)]
struct Piece {
    m: Mask,
    color: Rgb,
    alpha: i32,
    gap_after: i32,
    anchor: i32,
    bx: i32,
    by: i32,
    bar_pct: i32,
    bar_color: Rgb,
    halo: Mask,
    halo_r: i32,
    halo_alpha: i32,
    px: i32,
    py: i32,
}

fn piece(m: Mask, color: Rgb, alpha: i32, gap_after: i32) -> Piece {
    Piece { m, color, alpha, gap_after, anchor: -1, bx: 0, by: 0, bar_pct: -1, bar_color: Rgb::default(), halo: Mask::default(),
            halo_r: 0, halo_alpha: 0, px: i32::MIN, py: 0 }
}

/// Drei Anordnungen: Zeile (Standard), gestapelt (Symbol über Prozent), senkrecht (linke/rechte Taskleiste).
fn layout(bar_w: i32, bar_h: i32, width: &mut i32, height: &mut i32) -> Vec<Piece> {
    let s = g();
    let light = s.light;
    let fg = if light { rgbc(0, 0, 0) } else { rgbc(255, 255, 255) };
    let red = if light { rgbc(196, 30, 40) } else { rgbc(255, 90, 90) };
    let charge = if light { rgbc(0, 120, 212) } else { rgbc(96, 205, 255) };
    let full = if light { rgbc(16, 124, 16) } else { rgbc(108, 203, 95) };
    let icons = make_font(px(16), FW_NORMAL as i32, "Segoe Fluent Icons", "Segoe MDL2 Assets", 0);
    let bolt = make_font(px(11), FW_NORMAL as i32, "Segoe Fluent Icons", "Segoe MDL2 Assets", 0);
    let text = make_font(px(13), FW_NORMAL as i32, "Segoe UI Variable Text", "Segoe UI", px(12));
    let caption = make_font(8.max(px(7)), FW_SEMIBOLD as i32, "Segoe UI Variable Text", "Segoe UI", 0);

    struct Slot {
        icon: usize,
        label: usize,
        bolt: i32,
    }
    let mut out: Vec<Piece> = Vec::new();
    let mut slots: Vec<Slot> = Vec::new();
    if s.items.is_empty() {
        out.push(piece(render_mask(&[BATTERY_UNKNOWN], icons, light), fg, 110, 0));
    }
    let n_items = s.items.len();
    for (i, it) in s.items.iter().enumerate() {
        let a = if it.dim { 120 } else { 255 };
        let tc = if it.low { red } else { fg };
        let mark = it.charging || it.full;
        let gi = out.len();
        slots.push(Slot { icon: gi, label: gi + 1, bolt: if mark { gi as i32 + 2 } else { -1 } });
        let branded = it.accent != Accent::None;
        let (ar, ag, ab) = accent_color(it.accent, light);
        let accent = rgbc(ar as i32, ag as i32, ab as i32);
        let mut icon = piece(render_mask(&[glyph(it.kind, it.bluetooth)], icons, light), if branded { accent } else { fg }, a, px(5));
        if branded && !it.dim {
            icon.halo_r = px(3);
            icon.halo = halo(&icon.m, icon.halo_r);
            icon.halo_alpha = if light { 60 } else { 140 };
        }
        out.push(icon);
        let tstr = if !it.text.is_empty() {
            it.text.clone()
        } else if it.percent >= 0 {
            format!("{}%", it.percent)
        } else {
            "?".into()
        };
        let tw: Vec<u16> = tstr.encode_utf16().collect();
        let mut label = piece(render_mask(&tw, text, light), tc, a, if mark { px(2) } else { 0 });
        if branded && it.percent >= 0 {
            label.bar_pct = it.percent;
            label.bar_color = if it.low { red } else { accent };
        }
        out.push(label);
        if mark {
            out.push(piece(render_mask(&[if it.full { CHECK } else { BOLT }], bolt, light), if it.full { full } else { charge }, a, 0));
        }
        out.last_mut().unwrap().gap_after = if i + 1 < n_items { px(14) } else { 0 };
        if !it.label.is_empty() {
            let lw: Vec<u16> = it.label.encode_utf16().collect();
            let mut c = piece(render_mask(&lw, caption, light), fg, a, 0);
            c.anchor = gi as i32;
            c.bx = out[gi].m.w - c.m.w + px(3);
            c.by = out[gi].m.h - c.m.h + px(3);
            out.push(c);
        }
    }
    unsafe {
        DeleteObject(icons as _);
        DeleteObject(bolt as _);
        DeleteObject(text as _);
        DeleteObject(caption as _);
    }

    *height = bar_h;
    let row_gap = px(2);
    let bar_room = px(5);
    let (mut icon_h, mut text_h) = (0, 0);
    for sl in &slots {
        icon_h = icon_h.max(out[sl.icon].m.h);
        text_h = text_h.max(out[sl.label].m.h);
    }
    let stacked = !s.vertical && s.stacked && !slots.is_empty() && bar_h >= icon_h + row_gap + text_h + bar_room + px(6);
    if s.vertical {
        *width = bar_w;
        let mut y = px(6);
        if slots.is_empty() {
            let ic = &mut out[0];
            ic.px = (bar_w - ic.m.w) / 2;
            ic.py = y;
            *height = y + ic.m.h + px(6);
            return out;
        }
        for sl in &slots {
            let bolt_w = if sl.bolt >= 0 { px(2) + out[sl.bolt as usize].m.w } else { 0 };
            let (iw, ih) = (out[sl.icon].m.w, out[sl.icon].m.h);
            out[sl.icon].px = (bar_w - iw) / 2;
            out[sl.icon].py = y;
            y += ih + row_gap;
            let (lw, lh, lbar) = (out[sl.label].m.w, out[sl.label].m.h, out[sl.label].bar_pct);
            out[sl.label].px = (bar_w - lw - bolt_w) / 2;
            out[sl.label].py = y;
            if sl.bolt >= 0 {
                let lpx = out[sl.label].px;
                let b = &mut out[sl.bolt as usize];
                b.px = lpx + lw + px(2);
                b.py = y + (lh - b.m.h) / 2;
            }
            y += lh + if lbar >= 0 { bar_room } else { 0 } + px(8);
        }
        *height = y;
        return out;
    }
    if stacked {
        let top = (bar_h - (icon_h + row_gap + text_h + bar_room)) / 2;
        let mut x = px(8);
        for sl in &slots {
            let bolt_w = if sl.bolt >= 0 { px(2) + out[sl.bolt as usize].m.w } else { 0 };
            let (iw, ih) = (out[sl.icon].m.w, out[sl.icon].m.h);
            let lw = out[sl.label].m.w;
            let slot_w = (iw + bolt_w).max(lw);
            out[sl.icon].px = x + (slot_w - iw - bolt_w) / 2;
            out[sl.icon].py = top + (icon_h - ih) / 2;
            if sl.bolt >= 0 {
                let ipx = out[sl.icon].px;
                let b = &mut out[sl.bolt as usize];
                b.px = ipx + iw + px(2);
                b.py = top + (icon_h - b.m.h) / 2;
            }
            out[sl.label].px = x + (slot_w - lw) / 2;
            out[sl.label].py = top + icon_h + row_gap;
            x += slot_w + px(10);
        }
        *width = x - px(10) + px(8);
        return out;
    }
    *width = px(8) * 2;
    for p in &out {
        if p.anchor < 0 {
            *width += p.m.w + p.gap_after;
        }
    }
    out
}

/// Zeichnet den Streifen für ein Fenster (Haupt-Taskleiste oder Kopie).
fn paint_strip(wnd: HWND, taskbar: HWND, hover: bool) -> SIZE {
    let s = g();
    let mut bar = rect0();
    unsafe { GetClientRect(taskbar, &mut bar) };
    let (mut ww, mut hh) = (0, 0);
    let pieces = layout(bar.right - bar.left, bar.bottom - bar.top, &mut ww, &mut hh);
    if ww <= 0 || hh <= 0 {
        return SIZE { cx: 0, cy: 0 };
    }
    let mut cv = Canvas::new(ww, hh);
    for p in cv.px.iter_mut() {
        *p = 0x01000000;
    }
    let light = s.light;
    let round_rect = |cv: &mut Canvas, inset: i32, r: i32, c: Rgb, a: i32| {
        for y in inset..hh - inset {
            for x in 0..ww {
                let dx = if x < r { r - x } else if x >= ww - r { x - (ww - r - 1) } else { 0 };
                let dy = if y < inset + r { inset + r - y } else if y >= hh - inset - r { y - (hh - inset - r - 1) } else { 0 };
                if dx * dx + dy * dy <= r * r {
                    cv.blend(x, y, c, a);
                }
            }
        }
    };
    let bar_color = if light { rgbc(243, 243, 243) } else { rgbc(32, 32, 32) };
    if s.background {
        round_rect(&mut cv, px(4), px(6), bar_color, 240);
    }
    if hover {
        round_rect(&mut cv, px(4), px(4), if light { rgbc(0, 0, 0) } else { rgbc(255, 255, 255) }, if light { 20 } else { 26 });
    }

    let mut x = px(8);
    let track = if light { rgbc(0, 0, 0) } else { rgbc(255, 255, 255) };
    let mut xs = vec![0; pieces.len()];
    let mut ys = vec![0; pieces.len()];
    for (i, p) in pieces.iter().enumerate() {
        if p.anchor < 0 {
            let fixed = p.px != i32::MIN;
            xs[i] = if fixed { p.px } else { x };
            ys[i] = if fixed { p.py } else { (hh - p.m.h) / 2 };
            if p.halo_r != 0 {
                cv.draw(&p.halo, xs[i] - p.halo_r, ys[i] - p.halo_r, p.color, p.halo_alpha * p.alpha / 255);
            }
            if p.bar_pct >= 0 {
                let bh = 2.max(px(2));
                let gap = px(3);
                if !fixed {
                    ys[i] = (hh - (p.m.h + gap + bh)) / 2;
                }
                let by = ys[i] + p.m.h + gap;
                let fill = (if p.bar_pct > 0 { 1 } else { 0 }).max((p.bar_pct * p.m.w + 50) / 100);
                for yy in by..by + bh {
                    for xx in 0..p.m.w {
                        if xx < fill {
                            cv.blend(xs[i] + xx, yy, p.bar_color, p.alpha);
                        } else {
                            cv.blend(xs[i] + xx, yy, track, 45 * p.alpha / 255);
                        }
                    }
                }
            }
            if !fixed {
                x += p.m.w + p.gap_after;
            }
        } else {
            let a = p.anchor as usize;
            xs[i] = xs[a] + p.bx;
            ys[i] = ys[a] + p.by;
            for oy in -1..=1 {
                for ox in -1..=1 {
                    if ox != 0 || oy != 0 {
                        cv.draw(&p.m, xs[i] + ox, ys[i] + oy, bar_color, 255);
                    }
                }
            }
        }
        cv.draw(&p.m, xs[i], ys[i], p.color, p.alpha);
    }

    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(screen);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = ww;
        bi.bmiHeader.biHeight = -hh;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(mem, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if !bmp.is_null() && !bits.is_null() {
            std::ptr::copy_nonoverlapping(cv.px.as_ptr(), bits as *mut u32, cv.px.len());
            let ob = SelectObject(mem, bmp as _);
            let sz = SIZE { cx: ww, cy: hh };
            let src = POINT { x: 0, y: 0 };
            let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            UpdateLayeredWindow(wnd, screen, std::ptr::null(), &sz, mem, &src, 0, &bf, ULW_ALPHA);
            SelectObject(mem, ob);
            DeleteObject(bmp as _);
        }
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
    }
    SIZE { cx: ww, cy: hh }
}

fn paint() {
    let s = g();
    if s.wnd.is_null() {
        return;
    }
    let sz = paint_strip(s.wnd, s.taskbar, s.hover);
    if sz.cx > 0 {
        s.size = sz;
    }
    s.last_w = -1;
}

fn paint_mirror(i: usize) {
    let s = g();
    let m = s.mirrors[i];
    s.dpi_bar = m.bar;
    let d = dpi();
    let sz = paint_strip(m.wnd, m.bar, m.hover);
    s.dpi_bar = std::ptr::null_mut();
    let mm = &mut s.mirrors[i];
    mm.dpi = d;
    if sz.cx > 0 {
        mm.size = sz;
    }
    mm.last_left = NOT_PLACED;
}

fn hide_mirror(i: usize) {
    let m = &mut g().mirrors[i];
    if m.last_left != NOT_PLACED || unsafe { IsWindowVisible(m.wnd) } != 0 {
        unsafe { ShowWindow(m.wnd, SW_HIDE) };
    }
    m.last_left = NOT_PLACED;
}

fn taskbar_offset(name: &str) -> Option<i32> {
    reg_get_dword(SETTINGS_KEY, name).map(|v| v as i32)
}

/// Kopie links neben die Uhr der Zweit-Taskleiste setzen, oder verstecken, wenn der Platz unklar ist.
fn position_mirror(i: usize) {
    let s = g();
    let mut bar = rect0();
    let mbar = s.mirrors[i].bar;
    unsafe {
        if GetWindowRect(mbar, &mut bar) == 0 || (GetWindowLongW(mbar, GWL_EXSTYLE) as u32 & WS_EX_LAYOUTRTL) != 0 {
            hide_mirror(i);
            return;
        }
    }
    s.dpi_bar = mbar;
    if dpi() != s.mirrors[i].dpi || s.mirrors[i].size.cy != bar.bottom - bar.top {
        paint_mirror(i);
        s.dpi_bar = mbar;
    }
    let mut right = 0;
    let mut min_left = bar.left;
    let mut known = false;
    let msize = s.mirrors[i].size;
    unsafe {
        let tray_cls = w("TrayNotifyWnd");
        let tray = FindWindowExW(mbar, std::ptr::null_mut(), tray_cls.as_ptr(), std::ptr::null());
        let mut tr = rect0();
        if !tray.is_null() && IsWindowVisible(tray) != 0 && GetWindowRect(tray, &mut tr) != 0 && tr.left > bar.left && tr.right <= bar.right
            && tr.right > tr.left && tr.bottom > bar.top && tr.top < bar.bottom
        {
            right = tr.left;
            known = true;
        } else if !is_xaml_taskbar(mbar) {
            let clock_cls = w("ClockButton");
            let clock = FindWindowExW(mbar, std::ptr::null_mut(), clock_cls.as_ptr(), std::ptr::null());
            let mut cr = rect0();
            right = if !clock.is_null() && IsWindowVisible(clock) != 0 && GetWindowRect(clock, &mut cr) != 0 { cr.left } else { bar.right - px(8) };
            for cls in ["ShellFeedsTaskbarViewWindow", "DynamicContent2"] {
                let cw = w(cls);
                let feeds = FindWindowExW(mbar, std::ptr::null_mut(), cw.as_ptr(), std::ptr::null());
                let mut fr = rect0();
                if !feeds.is_null() && IsWindowVisible(feeds) != 0 && GetWindowRect(feeds, &mut fr) != 0 && fr.left < right && fr.right > fr.left {
                    right = fr.left;
                }
            }
            known = true;
        } else if let Ok(wb) = WB.lock() {
            for sp in wb.spans.iter().filter(|sp| sp.bar == mbar as isize && sp.measured) {
                right = if sp.has_clock { sp.clock_left } else { bar.right - px(8) };
                if sp.has_widgets && sp.widgets.0 < right && sp.widgets.2 > right - msize.cx - px(2) {
                    right = sp.widgets.0;
                }
                if sp.has_tasks {
                    min_left = sp.tasks_right + px(4);
                }
                known = true;
            }
        }
    }
    if let Some(o) = taskbar_offset("TaskbarOffset") {
        right -= px(o);
    }
    if let Some(o) = taskbar_offset("SecondaryTaskbarOffset") {
        right -= px(o);
    }
    let left = right - msize.cx - px(2);
    s.dpi_bar = std::ptr::null_mut();
    if !known || msize.cx <= 0 || left < min_left {
        hide_mirror(i);
        return;
    }
    let mut p = POINT { x: left, y: bar.top };
    unsafe {
        ScreenToClient(mbar, &mut p);
        let m = &mut s.mirrors[i];
        let covered = !GetWindow(m.wnd, GW_HWNDPREV).is_null();
        if !covered && p.x == m.last_left {
            return;
        }
        m.last_left = p.x;
        SetWindowPos(m.wnd, HWND_TOP, p.x, 0, m.size.cx, m.size.cy, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

fn tool_info(id: HWND) -> TTTOOLINFOW {
    let s = g();
    let mut ti: TTTOOLINFOW = unsafe { std::mem::zeroed() };
    ti.cbSize = std::mem::size_of::<TTTOOLINFOW>() as u32;
    ti.uFlags = TTF_IDISHWND | TTF_SUBCLASS;
    ti.hwnd = s.notify;
    ti.uId = id as usize;
    ti
}

fn add_tool(wnd: HWND) {
    let s = g();
    if s.tip.is_null() {
        return;
    }
    if s.tip_text.is_empty() {
        s.tip_text = w(APP_NAME);
    }
    let mut ti = tool_info(wnd);
    ti.lpszText = s.tip_text.as_mut_ptr();
    unsafe { SendMessageW(s.tip, TTM_ADDTOOLW, 0, &ti as *const _ as LPARAM) };
}

fn destroy_mirror(m: &Mirror) {
    let s = g();
    unsafe {
        if !s.tip.is_null() {
            let ti = tool_info(m.wnd);
            SendMessageW(s.tip, TTM_DELTOOLW, 0, &ti as *const _ as LPARAM);
        }
        if !m.wnd.is_null() && IsWindow(m.wnd) != 0 {
            DestroyWindow(m.wnd);
        }
    }
}

fn secondary_taskbars() -> Vec<HWND> {
    let mut out = Vec::new();
    let cls = w("Shell_SecondaryTrayWnd");
    unsafe {
        let mut t = FindWindowExW(std::ptr::null_mut(), std::ptr::null_mut(), cls.as_ptr(), std::ptr::null());
        while !t.is_null() {
            let mut r = rect0();
            if IsWindowVisible(t) != 0 && GetWindowRect(t, &mut r) != 0 && r.right - r.left > r.bottom - r.top {
                out.push(t);
            }
            t = FindWindowExW(std::ptr::null_mut(), t, cls.as_ptr(), std::ptr::null());
        }
    }
    out
}

fn monitor_id(bar: HWND) -> String {
    unsafe {
        let mon = MonitorFromWindow(bar, MONITOR_DEFAULTTONULL);
        let mut mi: MONITORINFOEXW = std::mem::zeroed();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if mon.is_null() || GetMonitorInfoW(mon, &mut mi as *mut _ as *mut MONITORINFO) == 0 {
            return String::new();
        }
        let mut dd: DISPLAY_DEVICEW = std::mem::zeroed();
        dd.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
        if EnumDisplayDevicesW(mi.szDevice.as_ptr(), 0, &mut dd, EDD_GET_DEVICE_INTERFACE_NAME) != 0 && dd.DeviceID[0] != 0 {
            return from_w(&dd.DeviceID);
        }
        from_w(&mi.szDevice)
    }
}

fn monitor_label(bar: HWND) -> String {
    unsafe {
        let mon = MonitorFromWindow(bar, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFOEXW = std::mem::zeroed();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(mon, &mut mi as *mut _ as *mut MONITORINFO) == 0 {
            return t("Other monitor");
        }
        let dev = from_w(&mi.szDevice);
        let mut label = match dev.find("DISPLAY") {
            Some(d) => tf("Display {number}", &[("number", dev[d + 7..].to_string())]),
            None => t("Other monitor"),
        };
        let mut pm: MONITORINFOEXW = std::mem::zeroed();
        pm.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY), &mut pm as *mut _ as *mut MONITORINFO) != 0 {
            let (a, b) = (mi.monitorInfo.rcMonitor, pm.monitorInfo.rcMonitor);
            let dx = (a.left + a.right - b.left - b.right) / 2;
            let dy = (a.top + a.bottom - b.top - b.bottom) / 2;
            let side = if dx.abs() >= dy.abs() { if dx < 0 { "left" } else { "right" } } else if dy < 0 { "above" } else { "below" };
            label += &format!(" ({})", t(side));
        }
        label
    }
}

fn set_primary_hidden(hide: bool) {
    let s = g();
    if hide == s.primary_hidden || s.wnd.is_null() {
        return;
    }
    s.primary_hidden = hide;
    if hide {
        unsafe {
            ShowWindow(s.wnd, SW_HIDE);
            if !s.rebar.is_null() && IsWindow(s.rebar) != 0 {
                let mut r = rect0();
                GetClientRect(s.taskbar, &mut r);
                PostMessageW(s.taskbar, WM_SIZE, SIZE_RESTORED as usize, make_lparam(r.right, r.bottom));
            }
        }
    }
    s.last_w = -1;
}

fn make_lparam(lo: i32, hi: i32) -> LPARAM {
    ((hi as u32 & 0xFFFF) << 16 | (lo as u32 & 0xFFFF)) as LPARAM
}

fn sync_mirrors() {
    let s = g();
    let all = if s.wnd.is_null() { vec![] } else { secondary_taskbars() };
    let mut bars: Vec<HWND> = Vec::new();
    let mut chosen_found = false;
    if s.choice == "all" {
        bars = all;
    } else if s.choice != "main" {
        if let Some(b) = all.into_iter().find(|&b| monitor_id(b) == s.choice) {
            bars.push(b);
            chosen_found = true;
        }
    }
    set_primary_hidden(chosen_found);
    let mut i = 0;
    while i < s.mirrors.len() {
        let m = s.mirrors[i];
        if !bars.contains(&m.bar) || unsafe { IsWindow(m.wnd) } == 0 {
            destroy_mirror(&m);
            s.mirrors.remove(i);
        } else {
            i += 1;
        }
    }
    let cls = w(CLASS);
    let empty = w("");
    for b in bars {
        if s.mirrors.iter().any(|m| m.bar == b) {
            continue;
        }
        let wnd = unsafe {
            CreateWindowExW(WS_EX_LAYERED | WS_EX_NOACTIVATE, cls.as_ptr(), empty.as_ptr(), WS_CHILD | WS_CLIPSIBLINGS, 0, 0, 1, 1, b,
                            std::ptr::null_mut(), s.inst, std::ptr::null())
        };
        if wnd.is_null() {
            continue;
        }
        add_tool(wnd);
        s.mirrors.push(Mirror { bar: b, wnd, hover: false, size: SIZE { cx: 0, cy: 0 }, dpi: 0, last_left: NOT_PLACED });
        paint_mirror(s.mirrors.len() - 1);
    }
    set_measure_mirrors(!s.mirrors.is_empty());
    for i in 0..s.mirrors.len() {
        position_mirror(i);
    }
}

fn paint_all() {
    paint();
    for i in 0..g().mirrors.len() {
        paint_mirror(i);
    }
}

fn set_hover(h: HWND, on: bool) {
    let s = g();
    if h == s.wnd {
        if s.hover == on {
            return;
        }
        s.hover = on;
        paint();
        reposition();
        return;
    }
    for i in 0..s.mirrors.len() {
        if s.mirrors[i].wnd == h && s.mirrors[i].hover != on {
            s.mirrors[i].hover = on;
            paint_mirror(i);
            position_mirror(i);
        }
    }
}

unsafe extern "system" fn widget_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
            unsafe { TrackMouseEvent(&mut t) };
            set_hover(h, true);
            0
        }
        WM_MOUSELEAVE => {
            set_hover(h, false);
            0
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            let s = g();
            unsafe { PostMessageW(s.notify, s.click_msg, 0, 0) };
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        _ => unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
}

// ---------- Öffentliche Schnittstelle ----------

/// Widget auf der Haupt-Taskleiste anlegen. Klicks werden als `click_msg` an `notify` gemeldet.
pub fn create(inst: HINSTANCE, notify: HWND, click_msg: u32, light: bool) -> bool {
    destroy();
    let s = g();
    unsafe {
        let tray_wnd = w("Shell_TrayWnd");
        let tray_notify = w("TrayNotifyWnd");
        s.taskbar = FindWindowW(tray_wnd.as_ptr(), std::ptr::null());
        if s.taskbar.is_null() || FindWindowExW(s.taskbar, std::ptr::null_mut(), tray_notify.as_ptr(), std::ptr::null()).is_null() {
            s.taskbar = std::ptr::null_mut();
            return false;
        }
        let mut tb = rect0();
        GetWindowRect(s.taskbar, &mut tb);
        s.vertical = tb.bottom - tb.top > tb.right - tb.left;
        let rebar = w("ReBarWindow32");
        s.rebar = if is_xaml_taskbar(s.taskbar) {
            std::ptr::null_mut()
        } else {
            FindWindowExW(s.taskbar, std::ptr::null_mut(), rebar.as_ptr(), std::ptr::null())
        };
        s.notify = notify;
        s.click_msg = click_msg;
        s.light = light;
        s.inst = inst;
        let cls = w(CLASS);
        if !s.registered {
            let mut wc: WNDCLASSEXW = std::mem::zeroed();
            wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
            wc.lpfnWndProc = Some(widget_proc);
            wc.hInstance = inst;
            wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
            wc.lpszClassName = cls.as_ptr();
            s.registered = RegisterClassExW(&wc) != 0;
        }
        let empty = w("");
        s.wnd = CreateWindowExW(WS_EX_LAYERED | WS_EX_NOACTIVATE, cls.as_ptr(), empty.as_ptr(), WS_CHILD | WS_CLIPSIBLINGS, 0, 0, 1, 1,
                                s.taskbar, std::ptr::null_mut(), inst, std::ptr::null());
        if s.wnd.is_null() {
            s.taskbar = std::ptr::null_mut();
            return false;
        }
        let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_WIN95_CLASSES };
        InitCommonControlsEx(&icc);
        let tip_cls = w("tooltips_class32");
        s.tip = CreateWindowExW(WS_EX_TOPMOST, tip_cls.as_ptr(), std::ptr::null(), WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX, 0, 0, 0, 0,
                                notify, std::ptr::null_mut(), inst, std::ptr::null());
        if !s.tip.is_null() {
            apply_tip_theme();
            add_tool(s.wnd);
            SendMessageW(s.tip, TTM_SETMAXTIPWIDTH, 0, 600);
        }
    }
    watch_widgets_button(s.taskbar);
    s.last_dpi = dpi();
    paint();
    s.last_w = -1;
    reposition();
    if !s.primary_hidden {
        unsafe { ShowWindow(s.wnd, SW_SHOWNOACTIVATE) };
    }
    true
}

pub fn destroy() {
    let s = g();
    unsafe {
        if !s.rebar.is_null() && IsWindow(s.rebar) != 0 && !s.taskbar.is_null() && IsWindow(s.taskbar) != 0 {
            let mut r = rect0();
            GetClientRect(s.taskbar, &mut r);
            PostMessageW(s.taskbar, WM_SIZE, SIZE_RESTORED as usize, make_lparam(r.right, r.bottom));
        }
    }
    s.rebar = std::ptr::null_mut();
    let mirrors = std::mem::take(&mut s.mirrors);
    for m in &mirrors {
        destroy_mirror(m);
    }
    set_measure_mirrors(false);
    s.primary_hidden = false;
    if WB.lock().map(|w| w.wake != 0).unwrap_or(false) {
        watch_widgets_button(std::ptr::null_mut());
    }
    unsafe {
        if !s.tip.is_null() {
            DestroyWindow(s.tip);
            s.tip = std::ptr::null_mut();
        }
        if !s.wnd.is_null() {
            DestroyWindow(s.wnd);
            s.wnd = std::ptr::null_mut();
        }
    }
    s.taskbar = std::ptr::null_mut();
    s.hover = false;
}

pub fn alive() -> bool {
    let s = g();
    unsafe { !s.wnd.is_null() && IsWindow(s.wnd) != 0 && !s.taskbar.is_null() && IsWindow(s.taskbar) != 0 }
}

pub fn set_items(items: Vec<WidgetItem>, tooltip: &str) {
    let s = g();
    s.items = items;
    let new_tip = w(tooltip);
    if new_tip != s.tip_text && !s.tip.is_null() {
        s.tip_text = new_tip;
        let mut ti = tool_info(s.wnd);
        ti.lpszText = s.tip_text.as_mut_ptr();
        unsafe { SendMessageW(s.tip, TTM_UPDATETIPTEXTW, 0, &ti as *const _ as LPARAM) };
        for i in 0..s.mirrors.len() {
            let mut ti = tool_info(s.mirrors[i].wnd);
            ti.lpszText = s.tip_text.as_mut_ptr();
            unsafe { SendMessageW(s.tip, TTM_UPDATETIPTEXTW, 0, &ti as *const _ as LPARAM) };
        }
    }
    paint_all();
    reposition();
}

pub fn set_light_theme(light: bool) {
    g().light = light;
    apply_tip_theme();
    paint_all();
}

pub fn set_background(on: bool) {
    g().background = on;
    paint_all();
}

pub fn set_stacked(on: bool) {
    g().stacked = on;
    paint_all();
    if !g().wnd.is_null() {
        reposition();
    }
}

pub fn set_monitor_choice(choice: &str) {
    g().choice = if choice.is_empty() { "all".into() } else { choice.into() };
    sync_mirrors();
    if !g().wnd.is_null() {
        reposition();
    }
}

pub fn other_monitors() -> Vec<MonitorOption> {
    let mut v: Vec<MonitorOption> = secondary_taskbars().into_iter().map(|b| MonitorOption { id: monitor_id(b), label: monitor_label(b) }).collect();
    v.sort_by(|a, b| a.label.cmp(&b.label));
    v
}

/// Lage der Taskleiste prüfen und das Widget bei Bedarf verschieben; etwa einmal pro Sekunde aufrufen.
pub fn reposition() -> bool {
    if !alive() {
        return false;
    }
    sync_mirrors();
    let s = g();
    if s.primary_hidden {
        return true;
    }
    if dpi() != s.last_dpi {
        s.last_dpi = dpi();
        paint();
    }
    unsafe {
        let tray_cls = w("TrayNotifyWnd");
        let tray = FindWindowExW(s.taskbar, std::ptr::null_mut(), tray_cls.as_ptr(), std::ptr::null());
        if tray.is_null() {
            return true;
        }
        let (mut tr, mut bar) = (rect0(), rect0());
        GetWindowRect(tray, &mut tr);
        GetWindowRect(s.taskbar, &mut bar);
        let vertical = bar.bottom - bar.top > bar.right - bar.left;
        if vertical != s.vertical {
            s.vertical = vertical;
            paint();
        }
        let eq = |a: &RECT, b: &RECT| a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom;
        if s.vertical {
            let mut client = rect0();
            GetClientRect(s.taskbar, &mut client);
            let mut anchor = POINT { x: bar.left, y: tr.top };
            ScreenToClient(s.taskbar, &mut anchor);
            let mut top = anchor.y - s.size.cy - px(2);
            if let Some(o) = taskbar_offset("TaskbarOffset") {
                top -= px(o);
            }
            top = top.clamp(0, 0.max(client.bottom - s.size.cy));
            let left = 0.max((client.right - s.size.cx) / 2);
            if !s.rebar.is_null() && IsWindow(s.rebar) != 0 {
                let mut rr = rect0();
                GetWindowRect(s.rebar, &mut rr);
                let mut tl = POINT { x: rr.left, y: rr.top };
                ScreenToClient(s.taskbar, &mut tl);
                if tl.y < top && tl.y + (rr.bottom - rr.top) > top {
                    MoveWindow(s.rebar, tl.x, tl.y, rr.right - rr.left, top - tl.y, 1);
                }
            }
            let covered = !GetWindow(s.wnd, GW_HWNDPREV).is_null();
            if !covered && eq(&tr, &s.last_tray) && eq(&bar, &s.last_bar) && s.last_w == s.size.cy && s.last_left == top {
                return true;
            }
            s.last_tray = tr;
            s.last_bar = bar;
            s.last_w = s.size.cy;
            s.last_left = top;
            SetWindowPos(s.wnd, HWND_TOP, left, top, s.size.cx, s.size.cy, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            return true;
        }
        let rtl = (GetWindowLongW(s.taskbar, GWL_EXSTYLE) as u32 & WS_EX_LAYOUTRTL) != 0;
        let mut edge = if rtl { tr.right } else { tr.left };
        if !rtl {
            for cls in ["ShellFeedsTaskbarViewWindow", "DynamicContent2"] {
                let cw = w(cls);
                let feeds = FindWindowExW(s.taskbar, std::ptr::null_mut(), cw.as_ptr(), std::ptr::null());
                if feeds.is_null() || IsWindowVisible(feeds) == 0 {
                    continue;
                }
                let mut fr = rect0();
                GetWindowRect(feeds, &mut fr);
                if fr.left < edge && fr.right > fr.left {
                    edge = fr.left;
                }
            }
        }
        let wb = widgets_button_rect();
        if wb.right > wb.left && wb.top < bar.bottom && wb.bottom > bar.top
            && (if rtl { wb.left < edge + s.size.cx + px(2) && wb.right > edge } else { wb.right > edge - s.size.cx - px(2) && wb.left < edge })
        {
            edge = if rtl { wb.right } else { wb.left };
        }
        let mut p = POINT { x: edge, y: bar.top };
        ScreenToClient(s.taskbar, &mut p);
        let mut left = p.x - s.size.cx - px(2);
        if let Some(o) = taskbar_offset("TaskbarOffset") {
            left -= px(o);
        }
        if !s.rebar.is_null() && IsWindow(s.rebar) != 0 {
            let mut rr = rect0();
            GetWindowRect(s.rebar, &mut rr);
            let mut tl = POINT { x: if rtl { rr.right } else { rr.left }, y: rr.top };
            ScreenToClient(s.taskbar, &mut tl);
            if tl.x + (rr.right - rr.left) > left && left > tl.x {
                MoveWindow(s.rebar, tl.x, tl.y, left - tl.x, rr.bottom - rr.top, 1);
            }
        }
        let covered = !GetWindow(s.wnd, GW_HWNDPREV).is_null();
        if !covered && eq(&tr, &s.last_tray) && eq(&bar, &s.last_bar) && eq(&wb, &s.last_widgets) && s.last_w == s.size.cx && s.last_left == left {
            return true;
        }
        s.last_tray = tr;
        s.last_widgets = wb;
        s.last_bar = bar;
        s.last_w = s.size.cx;
        s.last_left = left;
        SetWindowPos(s.wnd, HWND_TOP, left, 0, s.size.cx, s.size.cy, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
    true
}
