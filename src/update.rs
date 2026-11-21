//! Stiller Updater: fragt GitHub nach dem neuesten Release, lädt den Installer im Hintergrund,
//! prüft seine SHA-256-Prüfsumme (von GitHub mitgeliefert) und meldet ihn dann zur Installation.

use crate::util::*;
use std::path::PathBuf;
use windows_sys::Win32::Networking::WinInet::*;

const API_URL: &str = "https://api.github.com/repos/fojadrachi/wireless-battery-monitor/releases/latest";
const DOWNLOAD_PREFIX: &str = "https://github.com/fojadrachi/wireless-battery-monitor/releases/download/";
const ASSET_PREFIX: &str = "Akku-Monitor-Setup-";

pub enum CheckResult {
    UpToDate,
    /// Installer liegt geprüft bereit: (Version, Pfad)
    Ready(String, PathBuf),
    Failed(String),
}

fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.trim_start_matches(['v', 'V']).split('.').map(|p| p.trim().parse::<u32>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)))
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Nur die installierte Version aktualisiert sich selbst (nicht der Build im Projektordner).
pub fn is_installed() -> bool {
    std::path::Path::new(&exe_path()).parent().map(|d| d.join("unins000.exe").exists()).unwrap_or(false)
}

fn http_get(url: &str, accept_json: bool, max: usize) -> Result<Vec<u8>, String> {
    unsafe {
        let agent = w(&format!("Akku-Monitor/{}", current_version()));
        let net = InternetOpenW(agent.as_ptr(), INTERNET_OPEN_TYPE_PRECONFIG, std::ptr::null(), std::ptr::null(), 0);
        if net.is_null() {
            return Err("InternetOpen".into());
        }
        let headers = w(if accept_json { "Accept: application/vnd.github+json\r\n" } else { "" });
        let u = w(url);
        let req = InternetOpenUrlW(net, u.as_ptr(), headers.as_ptr(), if accept_json { u32::MAX } else { 0 },
                                   INTERNET_FLAG_SECURE | INTERNET_FLAG_RELOAD | INTERNET_FLAG_NO_CACHE_WRITE | INTERNET_FLAG_NO_UI, 0);
        if req.is_null() {
            InternetCloseHandle(net);
            return Err("Verbindung fehlgeschlagen".into());
        }
        let mut status: u32 = 0;
        let mut len: u32 = 4;
        HttpQueryInfoW(req, HTTP_QUERY_STATUS_CODE | HTTP_QUERY_FLAG_NUMBER, &mut status as *mut u32 as *mut _, &mut len, std::ptr::null_mut());
        let mut out = Vec::new();
        if status == 200 {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let mut got: u32 = 0;
                if InternetReadFile(req, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut got) == 0 {
                    out.clear();
                    status = 0;
                    break;
                }
                if got == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..got as usize]);
                if out.len() > max {
                    status = 0;
                    break;
                }
            }
        }
        InternetCloseHandle(req);
        InternetCloseHandle(net);
        if status == 200 { Ok(out) } else { Err(format!("HTTP {status}")) }
    }
}

fn sha256_hex(data: &[u8]) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::*;
    unsafe {
        let mut hash = [0u8; 32];
        let st = BCryptHash(BCRYPT_SHA256_ALG_HANDLE, std::ptr::null(), 0, data.as_ptr(), data.len() as u32, hash.as_mut_ptr(), 32);
        (st == 0).then(|| hash.iter().map(|b| format!("{b:02x}")).collect())
    }
}

fn update_dir() -> Option<PathBuf> {
    let d = std::path::Path::new(&std::env::var_os("LOCALAPPDATA")?).join("AkkuMonitor").join("update");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

/// Prüft auf ein neueres Release und lädt es bei Bedarf herunter. Blockiert; im Hintergrund aufrufen.
pub fn check_and_download() -> CheckResult {
    let body = match http_get(API_URL, true, 2 * 1024 * 1024) {
        Ok(b) => b,
        Err(e) => return CheckResult::Failed(e),
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return CheckResult::Failed("ungültige Antwort".into());
    };
    let tag = json["tag_name"].as_str().unwrap_or_default().to_string();
    let (Some(latest), Some(current)) = (parse_version(&tag), parse_version(current_version())) else {
        return CheckResult::Failed("keine Versionsangabe".into());
    };
    if json["draft"].as_bool() == Some(true) || json["prerelease"].as_bool() == Some(true) || latest <= current {
        return CheckResult::UpToDate;
    }
    let Some(asset) = json["assets"].as_array().and_then(|a| {
        a.iter().find(|x| {
            let n = x["name"].as_str().unwrap_or_default();
            n.starts_with(ASSET_PREFIX) && n.to_lowercase().ends_with(".exe")
        })
    }) else {
        return CheckResult::Failed("kein Installer im Release".into());
    };
    let name = asset["name"].as_str().unwrap_or_default();
    let url = asset["browser_download_url"].as_str().unwrap_or_default();
    // Nur aus dem eigenen Repo laden, nur mit Prüfsumme.
    if !url.starts_with(DOWNLOAD_PREFIX) || name.contains(['/', '\\', ':']) {
        return CheckResult::Failed("unerwartete Download-Adresse".into());
    }
    let Some(digest) = asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(|d| d.to_lowercase()) else {
        return CheckResult::Failed("keine Prüfsumme".into());
    };
    let Some(dir) = update_dir() else { return CheckResult::Failed("kein Update-Ordner".into()) };
    let path = dir.join(name);
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    // Schon fertig geladen?
    if let Ok(existing) = std::fs::read(&path) {
        if sha256_hex(&existing).as_deref() == Some(digest.as_str()) {
            return CheckResult::Ready(version, path);
        }
    }
    let data = match http_get(url, false, 100 * 1024 * 1024) {
        Ok(d) => d,
        Err(e) => return CheckResult::Failed(e),
    };
    if data.len() < 100 * 1024 || !data.starts_with(b"MZ") || sha256_hex(&data).as_deref() != Some(digest.as_str()) {
        return CheckResult::Failed("Prüfsumme stimmt nicht".into());
    }
    let tmp = path.with_extension("part");
    if std::fs::write(&tmp, &data).is_err() || std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return CheckResult::Failed("Speichern fehlgeschlagen".into());
    }
    // Alte Installer aufräumen
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.path() != path {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    CheckResult::Ready(version, path)
}

/// Startet den geladenen Installer still; er schließt die App, aktualisiert und startet sie neu.
pub fn launch_installer(path: &std::path::Path) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let (op, file) = (w("open"), w(&path.display().to_string()));
    let args = w("/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /AKKUUPDATE=1");
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), file.as_ptr(), args.as_ptr(), std::ptr::null(), 0) };
    r as isize > 32
}
