//! First-run install: copy glint.exe to `%LOCALAPPDATA%\Programs\Glint` (GLINT-INSTALL).

use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use windows::Win32::UI::Shell::ShellExecuteW;
use std::sync::atomic::{AtomicIsize, AtomicU8, Ordering};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, COLORREF, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteObject, DrawTextW, FillRect, RoundRect, SelectObject,
    SetBkMode, SetTextColor, DT_CENTER, DT_SINGLELINE, DT_VCENTER, HBRUSH, HFONT, HGDIOBJ,
    PS_SOLID, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_FOCUS, ODS_SELECTED};
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForSystem, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetDlgCtrlID, GetDlgItem,
    GetMessageW, GetSystemMetrics, IsDialogMessageW, LoadCursorW, MessageBoxW, PostQuitMessage,
    RegisterClassExW, SendMessageW, SetForegroundWindow, ShowWindow, SW_SHOWNORMAL, TranslateMessage, BM_CLICK,
    BS_OWNERDRAW, HMENU, IDC_ARROW, IDYES, MB_ICONERROR, MB_ICONQUESTION, MB_OK, MB_YESNO, MSG, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW,
    WINDOW_STYLE, WM_COMMAND, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DRAWITEM, WM_KEYDOWN, WM_SETFONT,
    WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_EX_TOPMOST, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use crate::config::Config;
use crate::render::{font, rgb, BACKGROUND, CPU_HUE, SEPARATOR, TEXT_DIM};

/// The single-instance mutex, so a relaunch can release it before the child starts.
static INSTANCE: AtomicIsize = AtomicIsize::new(0);

pub fn hold_instance(handle: HANDLE) {
    INSTANCE.store(handle.0 as isize, Ordering::SeqCst);
}

pub fn release_instance() {
    let raw = INSTANCE.swap(0, Ordering::SeqCst);
    if raw != 0 {
        unsafe {
            let _ = CloseHandle(HANDLE(raw as *mut _));
        }
    }
}

fn install_dir() -> Option<PathBuf> {
    let mut path = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    path.push("Programs");
    path.push("Glint");
    Some(path)
}

pub fn is_installed() -> bool {
    let (Ok(exe), Some(dir)) = (std::env::current_exe(), install_dir()) else {
        return false;
    };
    let Some(here) = exe.parent() else {
        return false;
    };
    here.as_os_str().eq_ignore_ascii_case(dir.as_os_str())
}

/// Ask once on first run. `true` means glint relaunched from the install folder and this process should exit.
pub fn offer() -> bool {
    let mut config = Config::load();
    if config.install_declined {
        return false;
    }
    match ask() {
        Choice::Install => return install_and_relaunch(),
        Choice::Never => {
            config.install_declined = true;
            config.save();
        }
        Choice::Later => {}
    }
    false
}

#[derive(Clone, Copy, PartialEq)]
enum Choice {
    Install,
    Later,
    Never,
}

// Button ids. `Later` is 2 on purpose: Esc arrives as IDCANCEL (2), so it means "not now".
const ID_INSTALL: usize = 1;
const ID_LATER: usize = 2;
const ID_NEVER: usize = 3;
const ID_TITLE: usize = 10;
const ID_BODY: usize = 11;

const TEXT: COLORREF = rgb(0xF2, 0xF2, 0xF7);
const BUTTON_FILL: COLORREF = rgb(0x2C, 0x2C, 0x2E);
const BUTTON_FILL_PRESSED: COLORREF = rgb(0x3A, 0x3A, 0x3C);
const ACCENT_PRESSED: COLORREF = rgb(0x3A, 0x7C, 0xD0);

static CHOICE: AtomicU8 = AtomicU8::new(ID_LATER as u8);
static BACKGROUND_BRUSH: AtomicIsize = AtomicIsize::new(0);
static BOLD_FONT: AtomicIsize = AtomicIsize::new(0);
static BODY_FONT: AtomicIsize = AtomicIsize::new(0);

/// A small dark window in glint's own palette. `MessageBoxW` cannot name its buttons.
fn ask() -> Choice {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let Ok(instance) = GetModuleHandleW(None) else {
            return Choice::Later;
        };
        let instance: HINSTANCE = instance.into();
        let class = w!("glint.install");
        let background = CreateSolidBrush(BACKGROUND);
        BACKGROUND_BRUSH.store(background.0 as isize, Ordering::SeqCst);
        let wnd_class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(ask_proc),
            hInstance: instance,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: background,
            ..Default::default()
        };
        RegisterClassExW(&wnd_class);

        let dpi = GetDpiForSystem().max(96);
        let px = |value: i32| value * dpi as i32 / 96;
        let mut frame = RECT { left: 0, top: 0, right: px(420), bottom: px(190) };
        let style = WS_CAPTION | WS_SYSMENU;
        let _ = AdjustWindowRectExForDpi(&mut frame, style, false, WS_EX_TOPMOST, dpi);
        let (outer_w, outer_h) = (frame.right - frame.left, frame.bottom - frame.top);
        let Ok(window) = CreateWindowExW(
            WS_EX_TOPMOST,
            class,
            w!("Install Glint"),
            style,
            (GetSystemMetrics(SM_CXSCREEN) - outer_w) / 2,
            (GetSystemMetrics(SM_CYSCREEN) - outer_h) / 3,
            outer_w,
            outer_h,
            None,
            None,
            Some(instance),
            None,
        ) else {
            return Choice::Later;
        };
        let (round, dark) = (DWMWCP_ROUND.0, 1i32);
        let _ = DwmSetWindowAttribute(window, DWMWA_WINDOW_CORNER_PREFERENCE, (&raw const round).cast(), 4);
        let _ = DwmSetWindowAttribute(window, DWMWA_USE_IMMERSIVE_DARK_MODE, (&raw const dark).cast(), 4);

        let title_font = font(px(19), 600);
        let body_font = font(px(13), 400);
        let bold_font = font(px(13), 600);
        BODY_FONT.store(body_font.0 as isize, Ordering::SeqCst);
        BOLD_FONT.store(bold_font.0 as isize, Ordering::SeqCst);

        let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain(std::iter::once(0)).collect() };
        let body = wide("Glint copies itself to %LOCALAPPDATA%\\Programs\\Glint and runs from there. No admin rights needed. If you skip this, the tray menu still has \"Install for me\".");
        let child = |class: PCWSTR, label: PCWSTR, style: WINDOW_STYLE, id: usize, font: HFONT, rect: (i32, i32, i32, i32)| {
            if let Ok(control) = CreateWindowExW(
                Default::default(),
                class,
                label,
                WS_CHILD | WS_VISIBLE | style,
                px(rect.0),
                px(rect.1),
                px(rect.2),
                px(rect.3),
                Some(window),
                Some(HMENU(id as *mut _)),
                Some(instance),
                None,
            ) {
                SendMessageW(control, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
            }
        };
        child(w!("STATIC"), w!("Install Glint?"), WINDOW_STYLE(0), ID_TITLE, title_font, (24, 20, 372, 30));
        child(w!("STATIC"), PCWSTR(body.as_ptr()), WINDOW_STYLE(0), ID_BODY, body_font, (24, 58, 372, 60));
        let button = WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32);
        child(w!("BUTTON"), w!(""), button, ID_INSTALL, body_font, (24, 134, 130, 36));
        child(w!("BUTTON"), w!(""), button, ID_LATER, body_font, (164, 134, 96, 36));
        child(w!("BUTTON"), w!(""), button, ID_NEVER, body_font, (270, 134, 126, 36));

        let _ = ShowWindow(window, SW_SHOW);
        let _ = SetForegroundWindow(window);
        if let Ok(install) = GetDlgItem(Some(window), ID_INSTALL as i32) {
            let _ = SetFocus(Some(install));
        }
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            // Enter presses the focused button, not the dialog default.
            if message.message == WM_KEYDOWN && message.wParam.0 == VK_RETURN.0 as usize {
                if let Ok(focused) = GetDlgItem(Some(window), GetDlgCtrlID(message.hwnd)) {
                    SendMessageW(focused, BM_CLICK, None, None);
                    continue;
                }
            }
            if !IsDialogMessageW(window, &message).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        for object in [title_font, body_font, bold_font] {
            let _ = DeleteObject(object.into());
        }
    }
    match CHOICE.load(Ordering::SeqCst) as usize {
        ID_INSTALL => Choice::Install,
        ID_NEVER => Choice::Never,
        _ => Choice::Later,
    }
}

unsafe extern "system" fn ask_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_COMMAND => {
            let id = wparam.0 & 0xffff;
            if matches!(id, ID_INSTALL | ID_LATER | ID_NEVER) {
                CHOICE.store(id as u8, Ordering::SeqCst);
                unsafe {
                    let _ = DestroyWindow(window);
                }
            }
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let id = unsafe { GetDlgCtrlID(HWND(lparam.0 as *mut _)) } as usize;
            unsafe {
                SetTextColor(windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut _), if id == ID_TITLE { TEXT } else { TEXT_DIM });
                SetBkMode(windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut _), TRANSPARENT);
            }
            LRESULT(BACKGROUND_BRUSH.load(Ordering::SeqCst))
        }
        WM_DRAWITEM => {
            draw_button(unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) });
            LRESULT(1)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

fn draw_button(item: &DRAWITEMSTRUCT) {
    let id = item.CtlID as usize;
    let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
    let focused = item.itemState.0 & ODS_FOCUS.0 != 0;
    let (fill, border, color, label, bold) = match id {
        ID_INSTALL => (if pressed { ACCENT_PRESSED } else { CPU_HUE }, CPU_HUE, BACKGROUND, "Install for me", true),
        ID_LATER => (if pressed { BUTTON_FILL_PRESSED } else { BUTTON_FILL }, SEPARATOR, TEXT, "Not now", false),
        _ => (BACKGROUND, BACKGROUND, TEXT_DIM, "Don't ask again", false),
    };
    let border = if focused { TEXT } else { border };
    let font = HFONT(
        (if bold { BOLD_FONT.load(Ordering::SeqCst) } else { BODY_FONT.load(Ordering::SeqCst) }) as *mut _,
    );
    let mut text: Vec<u16> = label.encode_utf16().collect();
    let mut rect = item.rcItem;
    unsafe {
        let background = HBRUSH(BACKGROUND_BRUSH.load(Ordering::SeqCst) as *mut _);
        FillRect(item.hDC, &rect, background);
        let brush = CreateSolidBrush(fill);
        let pen = CreatePen(PS_SOLID, 1, border);
        let old_brush = SelectObject(item.hDC, HGDIOBJ(brush.0));
        let old_pen = SelectObject(item.hDC, HGDIOBJ(pen.0));
        let radius = (rect.bottom - rect.top) / 3;
        let _ = RoundRect(item.hDC, rect.left, rect.top, rect.right, rect.bottom, radius, radius);
        SelectObject(item.hDC, old_brush);
        SelectObject(item.hDC, old_pen);
        let _ = DeleteObject(brush.into());
        let _ = DeleteObject(pen.into());

        let old_font = SelectObject(item.hDC, HGDIOBJ(font.0));
        SetBkMode(item.hDC, TRANSPARENT);
        SetTextColor(item.hDC, color);
        DrawTextW(item.hDC, &mut text, &mut rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        SelectObject(item.hDC, old_font);
    }
}

/// Copy, re-point an existing Run value, start the copy. `true` means the caller should exit.
pub fn install_and_relaunch() -> bool {
    let (Ok(exe), Some(dir)) = (std::env::current_exe(), install_dir()) else {
        fail("Glint could not work out where to install itself.");
        return false;
    };
    let target = dir.join("glint.exe");
    if std::fs::create_dir_all(&dir).and_then(|()| std::fs::copy(&exe, &target)).is_err() {
        fail("Glint could not copy itself to %LOCALAPPDATA%\\Programs\\Glint.\n\nIf Glint is already running from there, exit it first.");
        return false;
    }
    crate::window::repoint_startup(&target);
    // The child would see "already running" and quit if the mutex were still held.
    release_instance();
    let path: Vec<u16> = target.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let started = unsafe { ShellExecuteW(None, w!("open"), PCWSTR(path.as_ptr()), None, None, SW_SHOWNORMAL) };
    if started.0 as usize <= 32 {
        fail("Glint was copied but could not start.");
        return false;
    }
    true
}

/// The tray menu item asks first: a stray click should not copy and restart the app.
pub fn confirm_install() -> bool {
    let body: Vec<u16> = "Install Glint to %LOCALAPPDATA%\\Programs\\Glint and restart it from there?"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe { MessageBoxW(None, PCWSTR(body.as_ptr()), w!("Install Glint"), MB_YESNO | MB_ICONQUESTION) == IDYES }
}

fn fail(text: &str) {
    let body: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(None, PCWSTR(body.as_ptr()), w!("Glint"), MB_OK | MB_ICONERROR);
    }
}
