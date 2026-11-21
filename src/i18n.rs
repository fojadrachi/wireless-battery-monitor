//! Übersetzungen aus einfachen Textdateien (Port von i18n.cpp aus BatteryHub).
//!
//! Eine Sprache ist eine UTF-8-Datei, eine Phrase pro Zeile, Englisch links:
//!     @code = de
//!     @name = Deutsch
//!     Refresh now = Jetzt aktualisieren
//! Eingebaut sind die Dateien aus `lang/`; dazu kommen `lang\*.txt` neben der Exe und
//! `%APPDATA%\AkkuMonitor\lang\*.txt`.

use crate::util::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

const BUILT_IN: &[(&str, &str)] = &[
    ("_template", include_str!("../lang/_template.txt")),
    ("ar", include_str!("../lang/ar.txt")),
    ("de", include_str!("../lang/de.txt")),
    ("es", include_str!("../lang/es.txt")),
    ("fa", include_str!("../lang/fa.txt")),
    ("fr", include_str!("../lang/fr.txt")),
    ("he", include_str!("../lang/he.txt")),
    ("hu", include_str!("../lang/hu.txt")),
    ("ja", include_str!("../lang/ja.txt")),
    ("ko", include_str!("../lang/ko.txt")),
    ("pl", include_str!("../lang/pl.txt")),
    ("pt-BR", include_str!("../lang/pt-BR.txt")),
    ("ru", include_str!("../lang/ru.txt")),
    ("sv", include_str!("../lang/sv.txt")),
    ("tr", include_str!("../lang/tr.txt")),
    ("uk", include_str!("../lang/uk.txt")),
    ("vi", include_str!("../lang/vi.txt")),
    ("zh-CN", include_str!("../lang/zh-CN.txt")),
    ("zh-TW", include_str!("../lang/zh-TW.txt")),
];

#[derive(Default, Clone)]
struct Source {
    code: String,
    name: String,
    rtl: bool,
    texts: Vec<String>,
}

#[derive(Default)]
struct State {
    sources: BTreeMap<String, Source>,
    phrases: HashMap<String, String>,
    choice: String,
    rtl: bool,
    template: String,
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State::default());
}

pub struct Language {
    pub code: String,
    pub name: String,
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some(n) => out.push(n),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}')
}

fn for_each_line(text: &str, mut f: impl FnMut(&str, &str)) {
    for line in text.split('\n') {
        let line = trim(line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(eq) = line.find(" = ") {
            f(trim(&line[..eq]), trim(&line[eq + 3..]));
        }
    }
}

fn add_source(st: &mut State, text: &str, fallback_code: &str) {
    let (mut code, mut name, mut rtl) = (fallback_code.to_string(), String::new(), false);
    for_each_line(text, |k, v| match k {
        "@code" => code = v.to_string(),
        "@name" => name = v.to_string(),
        "@rtl" => rtl = v == "1" || v.eq_ignore_ascii_case("yes") || v.eq_ignore_ascii_case("true"),
        _ => {}
    });
    if code.is_empty() || code.eq_ignore_ascii_case("en") {
        return;
    }
    let s = st.sources.entry(code.to_lowercase()).or_default();
    if s.code.is_empty() {
        s.code = code;
    }
    if !name.is_empty() {
        s.name = name;
    }
    if rtl {
        s.rtl = true;
    }
    s.texts.push(text.to_string());
}

pub fn user_folder() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|a| std::path::Path::new(&a).join("AkkuMonitor").join("lang"))
}

fn load_folder(st: &mut State, dir: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let fname = p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        if !fname.to_lowercase().ends_with(".txt") || fname.starts_with('_') {
            continue;
        }
        if e.metadata().map(|m| m.len() > 512 * 1024).unwrap_or(true) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&p) {
            let text = String::from_utf8_lossy(&bytes).to_string();
            add_source(st, &text, &fname[..fname.len() - 4]);
        }
    }
}

fn auto_code(st: &State) -> String {
    use windows_sys::Win32::Globalization::*;
    let (mut count, mut size) = (0u32, 0u32);
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, std::ptr::null_mut(), &mut size) == 0 || size == 0 {
            return String::new();
        }
        let mut buf = vec![0u16; size as usize];
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buf.as_mut_ptr(), &mut size) == 0 {
            return String::new();
        }
        for part in buf.split(|&c| c == 0).filter(|p| !p.is_empty()) {
            let tag = String::from_utf16_lossy(part).to_lowercase();
            if tag.starts_with("zh-") {
                let trad = tag.contains("hant") || tag.contains("-tw") || tag.contains("-hk") || tag.contains("-mo");
                let want = if trad { "zh-tw" } else { "zh-cn" };
                if st.sources.contains_key(want) {
                    return want.into();
                }
            }
            if st.sources.contains_key(&tag) {
                return tag;
            }
            let base = tag.split('-').next().unwrap_or("").to_string();
            if base == "en" {
                return String::new();
            }
            if st.sources.contains_key(&base) {
                return base;
            }
            if let Some(code) = st.sources.keys().find(|c| c.starts_with(&format!("{base}-"))) {
                return code.clone();
            }
        }
    }
    String::new()
}

fn apply(st: &mut State) {
    st.phrases.clear();
    st.rtl = false;
    let code = if st.choice.is_empty() { auto_code(st) } else { st.choice.to_lowercase() };
    let Some(src) = st.sources.get(&code).cloned() else { return };
    st.rtl = src.rtl;
    for text in &src.texts {
        for_each_line(text, |k, v| {
            if !k.is_empty() && !k.starts_with('@') && !v.is_empty() {
                st.phrases.insert(unescape(k), unescape(v));
            }
        });
    }
}

/// Sprachen suchen und die gespeicherte Wahl anwenden.
pub fn init() {
    ST.with(|st| {
        let mut st = st.borrow_mut();
        st.sources.clear();
        for (file, text) in BUILT_IN {
            if *file == "_template" {
                st.template = text.to_string();
            } else {
                add_source(&mut st, text, file);
            }
        }
        if let Some(dir) = std::path::Path::new(&exe_path()).parent() {
            load_folder(&mut st, &dir.join("lang"));
        }
        if let Some(dir) = user_folder() {
            load_folder(&mut st, &dir);
        }
        st.choice = reg_get_string(SETTINGS_KEY, "Language").unwrap_or_default();
        apply(&mut st);
    });
}

pub fn available() -> Vec<Language> {
    ST.with(|st| {
        let st = st.borrow();
        let mut v: Vec<Language> = st
            .sources
            .values()
            .map(|s| Language { code: s.code.clone(), name: if s.name.is_empty() { s.code.clone() } else { s.name.clone() } })
            .collect();
        v.sort_by_key(|l| l.name.to_lowercase());
        v.insert(0, Language { code: "en".into(), name: "English".into() });
        v
    })
}

pub fn choice() -> String {
    ST.with(|st| st.borrow().choice.clone())
}

pub fn set_choice(code: &str) {
    reg_set_string(SETTINGS_KEY, "Language", code);
    ST.with(|st| {
        let mut st = st.borrow_mut();
        st.choice = code.to_string();
        apply(&mut st);
    });
}

pub fn rtl() -> bool {
    ST.with(|st| st.borrow().rtl)
}

/// Legt %APPDATA%\AkkuMonitor\lang mit einer Vorlage aller Phrasen an und öffnet den Ordner.
pub fn open_user_folder() {
    let Some(dir) = user_folder() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let tmpl = ST.with(|st| st.borrow().template.clone());
    let _ = std::fs::write(dir.join("_template.txt"), tmpl);
    let (op, path) = (w("open"), w(&dir.display().to_string()));
    unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), path.as_ptr(), std::ptr::null(),
                                                     std::ptr::null(), windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL);
    }
}

/// Übersetzung einer englischen Phrase (oder die Phrase selbst).
pub fn t(en: &str) -> String {
    ST.with(|st| st.borrow().phrases.get(en).cloned().unwrap_or_else(|| en.to_string()))
}

/// `t()` mit benannten Platzhaltern: `tf("{name} is at {percent}%", &[("name", n), ("percent", "40")])`.
pub fn tf(en: &str, args: &[(&str, String)]) -> String {
    let mut s = t(en);
    for (name, value) in args {
        s = s.replace(&format!("{{{name}}}"), value);
    }
    s
}
