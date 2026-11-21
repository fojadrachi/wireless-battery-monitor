//! Kleine Win32-Hilfen: Wide-Strings, Registry, DPI.

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Registry::*;

pub const SETTINGS_KEY: &str = r"Software\AkkuMonitor";
pub const APP_NAME: &str = "Akku-Monitor";

/// UTF-16 mit abschließender Null.
pub fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16-Puffer (bis zur ersten Null) → String.
pub fn from_w(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Nullterminierten UTF-16-Zeiger lesen (z. B. lParam von WM_SETTINGCHANGE).
pub fn pwstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    unsafe {
        while *p.add(n) != 0 && n < 4096 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }
}

pub fn reg_get_dword(key: &str, name: &str) -> Option<u32> {
    let (k, n) = (w(key), w(name));
    let mut v: u32 = 0;
    let mut sz: u32 = 4;
    let rc = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(),
                     &mut v as *mut u32 as *mut _, &mut sz)
    };
    (rc == ERROR_SUCCESS).then_some(v)
}

pub fn reg_set_dword(key: &str, name: &str, v: u32) {
    let (k, n) = (w(key), w(name));
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), REG_DWORD, &v as *const u32 as *const _, 4) };
}

pub fn reg_get_string(key: &str, name: &str) -> Option<String> {
    let (k, n) = (w(key), w(name));
    let mut buf = vec![0u16; 1024];
    let mut sz: u32 = (buf.len() * 2 - 2) as u32;
    let rc = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(),
                     buf.as_mut_ptr() as *mut _, &mut sz)
    };
    (rc == ERROR_SUCCESS).then(|| from_w(&buf))
}

pub fn reg_set_string(key: &str, name: &str, v: &str) {
    let (k, n, d) = (w(key), w(name), w(v));
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), REG_SZ, d.as_ptr() as *const _, (d.len() * 2) as u32) };
}

/// REG_MULTI_SZ als Liste von Strings.
pub fn reg_get_multi(key: &str, name: &str) -> Vec<String> {
    let (k, n) = (w(key), w(name));
    let mut size: u32 = 0;
    unsafe {
        if RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), RRF_RT_REG_MULTI_SZ, std::ptr::null_mut(),
                        std::ptr::null_mut(), &mut size) != ERROR_SUCCESS || size == 0 || size > 256 * 1024
        {
            return vec![];
        }
        let mut buf = vec![0u16; size as usize / 2 + 2];
        if RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), RRF_RT_REG_MULTI_SZ, std::ptr::null_mut(),
                        buf.as_mut_ptr() as *mut _, &mut size) != ERROR_SUCCESS
        {
            return vec![];
        }
        let mut out = Vec::new();
        let mut i = 0;
        while i < buf.len() && buf[i] != 0 {
            let start = i;
            while i < buf.len() && buf[i] != 0 {
                i += 1;
            }
            out.push(String::from_utf16_lossy(&buf[start..i]));
            i += 1;
        }
        out
    }
}

pub fn reg_set_multi(key: &str, name: &str, items: &[String]) {
    let mut data: Vec<u16> = Vec::new();
    for s in items {
        data.extend(s.encode_utf16());
        data.push(0);
    }
    data.push(0);
    if data.len() == 1 {
        data.push(0);
    }
    let (k, n) = (w(key), w(name));
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), REG_MULTI_SZ, data.as_ptr() as *const _, (data.len() * 2) as u32) };
}

pub fn load_bool(name: &str, default: bool) -> bool {
    reg_get_dword(SETTINGS_KEY, name).map(|v| v != 0).unwrap_or(default)
}

pub fn save_bool(name: &str, on: bool) {
    reg_set_dword(SETTINGS_KEY, name, on as u32);
}

pub fn mul_div(v: i32, num: i32, den: i32) -> i32 {
    ((v as i64 * num as i64 + den as i64 / 2) / den as i64) as i32
}

pub fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

pub fn exe_path() -> String {
    let mut buf = vec![0u16; 1024];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) };
    String::from_utf16_lossy(&buf[..n as usize])
}

/// Taskleiste hell? (`SystemUsesLightTheme`)
pub fn read_light_taskbar() -> bool {
    reg_get_dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "SystemUsesLightTheme")
        .map(|v| v != 0)
        .unwrap_or(false)
}

pub fn loword(v: usize) -> u32 {
    (v & 0xFFFF) as u32
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn tick() -> u64 {
    unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
}

pub const fn rect0() -> RECT {
    RECT { left: 0, top: 0, right: 0, bottom: 0 }
}
