//! Kleiner Dialog "Gerät umbenennen" (Port von rename_dialog.cpp aus BatteryHub).

use crate::i18n::t;
use crate::model::aliases;
use crate::util::*;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

struct EditorState {
    initial: String,
    result: String,
    edit: HWND,
    accepted: bool,
}

const EM_SETSEL: u32 = 0x00B1;
const EM_SETLIMITTEXT: u32 = 0x00C5;

unsafe extern "system" fn editor_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lp as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            return 1;
        }
        let st = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut EditorState;
        if st.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let st = &mut *st;
        match msg {
            WM_CREATE => {
                let inst = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
                let d = GetDpiForWindow(hwnd) as i32;
                let sc = |v: i32| mul_div(v, d, 96);
                let (stat, edit, btn) = (w("STATIC"), w("EDIT"), w("BUTTON"));
                let lbl = w(&t("Device display name:"));
                let label = CreateWindowExW(0, stat.as_ptr(), lbl.as_ptr(), WS_CHILD | WS_VISIBLE, sc(16), sc(18), sc(382), sc(22), hwnd,
                                            std::ptr::null_mut(), inst, std::ptr::null());
                let init = w(&st.initial);
                st.edit = CreateWindowExW(WS_EX_CLIENTEDGE, edit.as_ptr(), init.as_ptr(), WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
                                          sc(16), sc(45), sc(382), sc(25), hwnd, 100 as HMENU, inst, std::ptr::null());
                let (save_t, cancel_t) = (w(&t("Save")), w(&t("Cancel")));
                let save = CreateWindowExW(0, btn.as_ptr(), save_t.as_ptr(), WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
                                           sc(220), sc(85), sc(85), sc(29), hwnd, IDOK as isize as HMENU, inst, std::ptr::null());
                let cancel = CreateWindowExW(0, btn.as_ptr(), cancel_t.as_ptr(), WS_CHILD | WS_VISIBLE | WS_TABSTOP, sc(313), sc(85), sc(85), sc(29),
                                             hwnd, IDCANCEL as isize as HMENU, inst, std::ptr::null());
                let font = GetStockObject(DEFAULT_GUI_FONT) as usize;
                for c in [label, st.edit, save, cancel] {
                    if !c.is_null() {
                        SendMessageW(c, WM_SETFONT, font, 1);
                    }
                }
                SendMessageW(st.edit, EM_SETLIMITTEXT, aliases::MAX_NAME_LENGTH, 0);
                SetFocus(st.edit);
                SendMessageW(st.edit, EM_SETSEL, 0, -1);
                0
            }
            WM_COMMAND => {
                let id = loword(wp) as i32;
                if id == IDOK {
                    let len = GetWindowTextLengthW(st.edit);
                    let mut buf = vec![0u16; len as usize + 1];
                    GetWindowTextW(st.edit, buf.as_mut_ptr(), buf.len() as i32);
                    let name = aliases::trim(&from_w(&buf));
                    if !aliases::valid_name(&name) {
                        let (m, c) = (w(&t("Enter a name (1-64 characters).")), w(&t("Rename device")));
                        MessageBoxW(hwnd, m.as_ptr(), c.as_ptr(), MB_OK | MB_ICONINFORMATION);
                        SetFocus(st.edit);
                        return 0;
                    }
                    st.result = name;
                    st.accepted = true;
                    DestroyWindow(hwnd);
                    return 0;
                }
                if id == IDCANCEL {
                    DestroyWindow(hwnd);
                    return 0;
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Modaler Namensdialog; `Some(name)` bei "Speichern".
pub fn prompt_device_name(owner: HWND, current: &str) -> Option<String> {
    unsafe {
        let inst = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
        let cls = w("AkkuMonitorRenameDialog");
        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc.hInstance = inst;
        wc.lpfnWndProc = Some(editor_proc);
        wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
        wc.hbrBackground = (COLOR_BTNFACE + 1) as isize as HBRUSH;
        wc.lpszClassName = cls.as_ptr();
        RegisterClassExW(&wc);

        let d = if owner.is_null() { 96 } else { GetDpiForWindow(owner) as i32 };
        let (width, height) = (mul_div(420, d, 96), mul_div(157, d, 96));
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        let mon = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let mut area = rect0();
        if GetMonitorInfoW(mon, &mut mi) != 0 {
            area = mi.rcWork;
        } else {
            SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut area as *mut _ as *mut _, 0);
        }
        let x = (area.left + (area.right - area.left - width) / 2).max(area.left);
        let y = (area.top + (area.bottom - area.top - height) / 2).max(area.top);

        let mut state = EditorState { initial: current.to_string(), result: String::new(), edit: std::ptr::null_mut(), accepted: false };
        let title = w(&t("Rename device"));
        let dialog = CreateWindowExW(WS_EX_DLGMODALFRAME | WS_EX_TOOLWINDOW, cls.as_ptr(), title.as_ptr(), WS_CAPTION | WS_SYSMENU | WS_POPUP,
                                     x, y, width, height, owner, std::ptr::null_mut(), inst, &mut state as *mut _ as *const _);
        if dialog.is_null() {
            return None;
        }
        let owner_enabled = !owner.is_null() && IsWindowEnabled(owner) != 0;
        if owner_enabled {
            EnableWindow(owner, 0);
        }
        ShowWindow(dialog, SW_SHOW);
        UpdateWindow(dialog);
        SetForegroundWindow(dialog);
        if !state.edit.is_null() {
            SetFocus(state.edit);
            SendMessageW(state.edit, EM_SETSEL, 0, -1);
        }
        let mut msg: MSG = std::mem::zeroed();
        while IsWindow(dialog) != 0 {
            let code = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if code <= 0 {
                if code == 0 {
                    PostQuitMessage(msg.wParam as i32);
                }
                break;
            }
            if IsDialogMessageW(dialog, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if IsWindow(dialog) != 0 {
            DestroyWindow(dialog);
        }
        if owner_enabled {
            EnableWindow(owner, 1);
        }
        state.accepted.then_some(state.result)
    }
}
