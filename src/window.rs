//! The tray icon, the popup window, and every interaction.
//!
//! The window is a `WS_POPUP` with `WS_EX_TOOLWINDOW`, so it owns no taskbar
//! button. DWM rounds its corners. All state lives in one `App`, and the
//! window procedure reaches it through `GWLP_USERDATA`.

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateBitmap, CreateDIBSection, EndPaint, GetDC, InvalidateRect, MonitorFromPoint,
    MonitorFromRect, MonitorFromWindow, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP, HBRUSH, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    MONITOR_DEFAULTTONULL, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, GetDpiForWindow, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER,
    NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DestroyWindow, DispatchMessageW, GetMessageW, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowRect, KillTimer, LoadCursorW, PostMessageW, PostQuitMessage,
    RegisterClassExW, RegisterWindowMessageW, SendMessageW, WINDOW_STYLE, WS_EX_TOPMOST, SetForegroundWindow, SetTimer, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, TrackPopupMenu, TranslateMessage, GWLP_USERDATA, HICON, HTCAPTION,
    HWND_TOPMOST, ICONINFO, IDC_ARROW, MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG,
    PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, SM_CXSMICON, SWP_NOACTIVATE,
    SW_HIDE, SW_SHOWNOACTIVATE, TPM_RIGHTBUTTON,
    WM_COMMAND, WM_NOTIFY, WM_CONTEXTMENU, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_NCLBUTTONDOWN, WM_PAINT,
    WM_POWERBROADCAST, WM_RBUTTONUP, WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_POPUP,
};

use windows::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX, NMTTDISPINFOW, TTTOOLINFOW,
    TOOLTIPS_CLASSW, TTF_SUBCLASS, TTM_ADDTOOLW, TTM_NEWTOOLRECTW, TTN_GETDISPINFOW, TTS_ALWAYSTIP,
    TTS_NOPREFIX,
};

use crate::config::Config;
use crate::render::{contains, Layout, Renderer};
use crate::sampler::{Histories, Metrics, Sampler};

/// The message the tray sends for icon events.
const WM_TRAY: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;
/// `NIN_SELECT | NINF_KEY`, which the `windows` crate does not export.
const NIN_KEYSELECT: u32 = NIN_SELECT | 0x1;
const TRAY_ID: u32 = 1;
const TIMER_POLL: usize = 1;
const TIMER_TRAY_RETRY: usize = 2;
const TRAY_RETRY_MS: u32 = 2000;

/// GLINT-IDLE-COST: a hidden window polls once per 10 s and never renders.
const POLL_VISIBLE_MS: u32 = 1000;
const POLL_HIDDEN_MS: u32 = 10_000;

/// The gap between the window and the edges of the work area.
const EDGE_GAP: i32 = 8;

const MENU_STARTUP: usize = 102;
const MENU_RESET_POSITION: usize = 103;
const MENU_EXIT: usize = 104;

pub struct App {
    window: HWND,
    sampler: Sampler,
    metrics: Metrics,
    history: Histories,
    renderer: Renderer,
    config: Config,
    layout: Layout,
    icon: HICON,
    visible: bool,
    /// True only between the drag that the user started and its end.
    ///
    /// `WM_EXITSIZEMOVE` also arrives for moves the app itself made, such as
    /// a `SetWindowPos` that crosses monitors. Storing the position on every
    /// one of those would pin the window to a place the user never chose, and
    /// the flyout would stop following the tray icon.
    dragging: bool,
    /// `TaskbarCreated`, which Explorer broadcasts after a restart.
    taskbar_created: u32,
    dpi: u32,
    /// The hover text control for the two header buttons.
    tooltip: HWND,
}

/// `TTTOOLINFOW` without `lpReserved`: the size every comctl32 version accepts.
const TOOLINFO_SIZE: u32 = (size_of::<TTTOOLINFOW>() - size_of::<*mut u8>()) as u32;

/// Tooltip tool id for the minimize button.
const TIP_MINIMIZE: usize = 1;

impl App {
    /// Build the window, the tray icon and the sampler.
    pub fn start() -> windows::core::Result<Box<Self>> {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }

        let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
        let class = w!("glint.window");
        let mut wnd_class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class,
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
            // The renderer paints every pixel, so no background brush.
            hbrBackground: HBRUSH::default(),
            ..Default::default()
        };
        wnd_class.cbSize = size_of::<WNDCLASSEXW>() as u32;
        unsafe { RegisterClassExW(&wnd_class) };

        let sampler = Sampler::new().map_err(|error| {
            windows::core::Error::new(windows::Win32::Foundation::E_FAIL, error.to_string())
        })?;
        let config = Config::load();

        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class,
                w!("Glint"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                Some(instance),
                None,
            )?
        };

        let dpi = unsafe { GetDpiForWindow(window) }.max(96);
        let mut app = Box::new(Self {
            window,
            layout: Layout::new(dpi, sampler.has_npu(), 0),
            sampler,
            metrics: Metrics::default(),
            history: Histories::default(),
            renderer: Renderer::new(),
            config,
            icon: make_tray_icon(),
            visible: false,
            dragging: false,
            taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
            dpi,
            tooltip: HWND::default(),
        });

        // The window procedure reads the app back out of the window.
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, (&raw mut *app) as isize);
        }

        app.apply_window_style();
        app.create_tooltips();
        app.add_tray_icon();
        app.set_poll_rate(POLL_HIDDEN_MS);
        Ok(app)
    }

    fn apply_window_style(&self) {
        let round = DWMWCP_ROUND.0;
        let dark: i32 = 1;
        unsafe {
            let _ = DwmSetWindowAttribute(
                self.window,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&raw const round).cast(),
                size_of::<i32>() as u32,
            );
            let _ = DwmSetWindowAttribute(
                self.window,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                (&raw const dark).cast(),
                size_of::<i32>() as u32,
            );
        }
    }

    // ------------------------------------------------------- the tray icon ----

    fn tray_data(&self, flags: windows::Win32::UI::Shell::NOTIFY_ICON_DATA_FLAGS) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.window,
            uID: TRAY_ID,
            uFlags: flags,
            uCallbackMessage: WM_TRAY,
            hIcon: self.icon,
            ..Default::default()
        }
    }

    /// Add the icon. The shell refuses the add while it is still starting, so
    /// a failure schedules one retry rather than leaving the app unreachable.
    fn add_tray_icon(&mut self) {
        let mut data = self.tray_data(NIF_MESSAGE | NIF_ICON | NIF_TIP);
        write_tip(&mut data.szTip, "Glint");
        let added = unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool();
        if !added {
            unsafe {
                SetTimer(Some(self.window), TIMER_TRAY_RETRY, TRAY_RETRY_MS, None);
            }
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.window), TIMER_TRAY_RETRY);
        }
        // Version 4 puts the click position in `wParam`, which the popup
        // needs in order to open next to the icon.
        let mut version = self.tray_data(Default::default());
        version.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        unsafe {
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &version);
        }
    }

    fn remove_tray_icon(&self) {
        let data = self.tray_data(Default::default());
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        }
    }

    /// Keep the tooltip current, so the numbers are readable while hidden.
    fn update_tooltip(&mut self) {
        let mut text = format!(
            "CPU {:.0}%  MEM {:.0}%",
            self.metrics.cpu, self.metrics.memory
        );
        if let Some(gpu) = self.metrics.gpu {
            text.push_str(&format!("  GPU {gpu:.0}%"));
        }
        if let Some(npu) = self.metrics.npu {
            text.push_str(&format!("  NPU {npu:.0}%"));
        }
        let mut data = self.tray_data(NIF_TIP);
        write_tip(&mut data.szTip, &text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }

    // ----------------------------------------------------- show and hide ----

    fn set_poll_rate(&self, period_ms: u32) {
        unsafe {
            SetTimer(Some(self.window), TIMER_POLL, period_ms, None);
        }
    }

    fn show(&mut self) {
        self.place();
        unsafe {
            let _ = ShowWindow(self.window, SW_SHOWNOACTIVATE);
        }
        self.visible = true;
        self.set_poll_rate(POLL_VISIBLE_MS);
        // Sample at once, so the window never opens on a stale reading.
        self.poll();
    }

    fn hide(&mut self) {
        unsafe {
            let _ = ShowWindow(self.window, SW_HIDE);
        }
        self.visible = false;
        self.set_poll_rate(POLL_HIDDEN_MS);
    }

    /// GLINT-TRAY-TOGGLE: one left click shows or hides the window.
    fn toggle(&mut self) {
        if self.visible {
            self.hide();
        } else {
            self.show();
        }
    }

    /// Re-measure at the current DPI. Nothing moves until `apply_bounds`.
    fn relayout(&mut self) {
        self.layout = Layout::new(
            self.dpi,
            self.sampler.has_npu(),
            self.metrics.volumes.len(),
        );
    }

    /// Move the window to `(x, y)` at the current layout size, on top.
    fn apply_bounds(&self, x: i32, y: i32) {
        unsafe {
            let _ = SetWindowPos(
                self.window,
                Some(HWND_TOPMOST),
                x,
                y,
                self.layout.width,
                self.layout.height,
                SWP_NOACTIVATE,
            );
            let _ = InvalidateRect(Some(self.window), None, false);
        }
        self.update_tooltips();
    }

    /// Delayed hover text for the header buttons. The control subclasses the
    /// window to watch the mouse, and asks for the text through
    /// `TTN_GETDISPINFOW` so the pin label always matches the pin state.
    fn create_tooltips(&mut self) {
        unsafe {
            let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
                dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_WIN95_CLASSES,
            });
            let Ok(tooltip) = CreateWindowExW(
                WS_EX_TOPMOST,
                TOOLTIPS_CLASSW,
                PCWSTR::null(),
                WS_POPUP | WINDOW_STYLE((TTS_ALWAYSTIP | TTS_NOPREFIX) as u32),
                0,
                0,
                0,
                0,
                Some(self.window),
                None,
                None,
                None,
            ) else {
                return;
            };
            self.tooltip = tooltip;
            for (id, rect) in [(TIP_MINIMIZE, self.layout.minimize_rect)] {
                let info = TTTOOLINFOW {
                    cbSize: TOOLINFO_SIZE,
                    uFlags: TTF_SUBCLASS,
                    hwnd: self.window,
                    uId: id,
                    rect,
                    lpszText: PWSTR(usize::MAX as *mut u16), // LPSTR_TEXTCALLBACKW
                    ..Default::default()
                };
                SendMessageW(tooltip, TTM_ADDTOOLW, None, Some(LPARAM((&raw const info) as isize)));
            }
        }
    }

    /// Keep the hover regions on the buttons after a resize or a DPI change.
    fn update_tooltips(&self) {
        if self.tooltip.0.is_null() {
            return;
        }
        for (id, rect) in [(TIP_MINIMIZE, self.layout.minimize_rect)] {
            let info = TTTOOLINFOW {
                cbSize: TOOLINFO_SIZE,
                hwnd: self.window,
                uId: id,
                rect,
                ..Default::default()
            };
            unsafe {
                SendMessageW(
                    self.tooltip,
                    TTM_NEWTOOLRECTW,
                    None,
                    Some(LPARAM((&raw const info) as isize)),
                );
            }
        }
    }

    /// Put the window at its saved position, or in the default corner when
    /// nothing is saved or the saved spot is on no connected monitor
    /// (GLINT-PLACEMENT).
    ///
    /// The layout is measured at the target monitor's scale before the move,
    /// so the window arrives at its final size.
    fn place(&mut self) {
        let saved = self.config.position.and_then(|(x, y)| {
            // Any part of the header on a monitor keeps the window reachable.
            let header = RECT {
                left: x,
                top: y,
                right: x + self.layout.width,
                bottom: y + self.layout.header_bottom,
            };
            let monitor = unsafe { MonitorFromRect(&header, MONITOR_DEFAULTTONULL) };
            (!monitor.0.is_null()).then_some((monitor, x, y))
        });
        let monitor = match saved {
            Some((monitor, _, _)) => monitor,
            // The monitor that owns the tray icon, so a multi-monitor desktop
            // opens the window on the screen the user clicked.
            None => unsafe {
                MonitorFromPoint(
                    self.tray_icon_point().unwrap_or_default(),
                    MONITOR_DEFAULTTONEAREST,
                )
            },
        };
        self.dpi = monitor_dpi(monitor).unwrap_or(self.dpi);
        self.relayout();
        let work = work_area(monitor);
        let (x, y) = match saved {
            Some((_, x, y)) => (x, keep_bottom_visible(y, self.layout.height, work)),
            None => self.default_corner(work),
        };
        self.apply_bounds(x, y);
    }

    /// The default corner: the lower right of the work area.
    ///
    /// The work area already excludes the taskbar, wherever the taskbar sits,
    /// so the window never covers it. The window lands in the same place
    /// every time rather than chasing an icon that the shell may move.
    fn default_corner(&self, work: Option<RECT>) -> (i32, i32) {
        let gap = (EDGE_GAP * self.dpi as i32) / 96;
        match work {
            Some(work) => (
                work.right - self.layout.width - gap,
                work.bottom - self.layout.height - gap,
            ),
            None => (gap, gap),
        }
    }

    /// Re-measure after the content changes size, without moving the window
    /// somewhere else.
    ///
    /// In the default corner the window re-anchors to the corner, so it grows
    /// upward. Anywhere else it keeps its top left and moves up only as far as
    /// it must to keep its bottom above the taskbar. During a drag it only
    /// changes size: moving it would fight the user's hand.
    fn refit(&mut self) {
        if self.config.position.is_none() && !self.dragging {
            self.place();
            return;
        }
        self.relayout();
        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.window, &mut rect);
        }
        let mut y = rect.top;
        if !self.dragging {
            let monitor = unsafe { MonitorFromWindow(self.window, MONITOR_DEFAULTTONEAREST) };
            y = keep_bottom_visible(y, self.layout.height, work_area(monitor));
        }
        self.apply_bounds(rect.left, y);
    }

    /// The centre of the tray icon, when the shell will say where it is.
    fn tray_icon_point(&self) -> Option<POINT> {
        let identifier = NOTIFYICONIDENTIFIER {
            cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.window,
            uID: TRAY_ID,
            ..Default::default()
        };
        let icon = unsafe { Shell_NotifyIconGetRect(&identifier) }.ok()?;
        Some(POINT {
            x: (icon.left + icon.right) / 2,
            y: (icon.top + icon.bottom) / 2,
        })
    }

    // ----------------------------------------------------------- the tick ----

    fn poll(&mut self) {
        self.metrics = self.sampler.tick();
        self.history.push(&self.metrics);
        self.update_tooltip();

        if !self.visible {
            // GLINT-IDLE-COST: no layout work and no rendering while hidden.
            return;
        }
        // A new disk count changes the height, so re-measure before painting.
        let wanted = Layout::new(
            self.dpi,
            self.sampler.has_npu(),
            self.metrics.volumes.len(),
        );
        if (wanted.width, wanted.height) != (self.layout.width, self.layout.height) {
            self.refit();
        }
        unsafe {
            let _ = InvalidateRect(Some(self.window), None, false);
        }
    }

    fn paint(&mut self) {
        let mut ps = PAINTSTRUCT::default();
        let dc = unsafe { BeginPaint(self.window, &mut ps) };
        if !dc.is_invalid() {
            self.renderer
                .paint(dc, &self.layout, &self.metrics, &self.history);
        }
        unsafe {
            let _ = EndPaint(self.window, &ps);
        }
    }

    // -------------------------------------------------------- interaction ----

    fn on_click(&mut self, point: POINT) {
        if contains(&self.layout.minimize_rect, point) {
            // GLINT-MINIMIZE: the window stays open until this, the tray
            // icon or the menu closes it.
            self.hide();
        }
    }

    fn show_menu(&mut self, at: POINT) {
        let menu = match unsafe { CreatePopupMenu() } {
            Ok(menu) => menu,
            Err(_) => return,
        };
        let startup_on = startup_enabled();
        let startup_flags = MF_STRING
            | if startup_on {
                MF_CHECKED
            } else {
                Default::default()
            };
        unsafe {
            let _ = AppendMenuW(menu, startup_flags, MENU_STARTUP, w!("Start with Windows"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_RESET_POSITION, w!("Reset position"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("Exit"));

            // The menu needs the foreground, or it will not dismiss properly.
            let _ = SetForegroundWindow(self.window);
            let _ = TrackPopupMenu(
                menu,
                TPM_RIGHTBUTTON,
                at.x,
                at.y,
                Some(0),
                self.window,
                None,
            );
            let _ = DestroyMenu(menu);
        }
    }

    fn on_command(&mut self, id: usize) {
        match id {
            MENU_STARTUP => set_startup_enabled(!startup_enabled()),
            MENU_RESET_POSITION => {
                self.config.position = None;
                self.config.save();
                if self.visible {
                    self.place();
                }
            }
            MENU_EXIT => unsafe {
                let _ = DestroyWindow(self.window);
            },
            _ => {}
        }
    }

    /// Remember where the user dragged the window. A move the app made itself
    /// is not a drag, and it leaves the stored position alone.
    fn store_position(&mut self) {
        if !self.dragging {
            return;
        }
        self.dragging = false;
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(self.window, &mut rect) }.is_ok() {
            self.config.position = Some((rect.left, rect.top));
            self.config.save();
        }
    }

    /// The window crossed onto a monitor with a different scale.
    ///
    /// During a drag, take the origin Windows suggests: it keeps the cursor
    /// on the same spot of the header. Re-placing the window here yanks it
    /// back mid-drag, which once stopped it crossing onto a monitor with a
    /// different scale. Outside a drag the app made the move itself and
    /// already chose the spot, so only the size changes.
    fn on_dpi_changed(&mut self, dpi: u32, suggested: RECT) {
        self.dpi = dpi.max(96);
        self.relayout();
        let (x, y) = if self.dragging {
            (suggested.left, suggested.top)
        } else {
            let mut rect = RECT::default();
            unsafe {
                let _ = GetWindowRect(self.window, &mut rect);
            }
            (rect.left, rect.top)
        };
        self.apply_bounds(x, y);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.remove_tray_icon();
    }
}

// ------------------------------------------------------ the message loop ----

/// Run the app until the window closes.
pub fn run() -> windows::core::Result<()> {
    let app = App::start()?;
    let mut message = MSG::default();
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    drop(app);
    Ok(())
}

/// Read the `App` back out of the window. Returns `None` before `start`
/// finishes storing it, which happens for the first few messages.
fn app_of(window: HWND) -> Option<&'static mut App> {
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) };
    if pointer == 0 {
        None
    } else {
        Some(unsafe { &mut *(pointer as *mut App) })
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let Some(app) = app_of(window) else {
        return unsafe { DefWindowProcW(window, message, wparam, lparam) };
    };

    if message == app.taskbar_created {
        // Explorer restarted and dropped every icon. Add ours again.
        app.add_tray_icon();
        app.update_tooltip();
        return LRESULT(0);
    }

    match message {
        WM_TRAY => {
            // Version 4 puts the event in the low word of `lparam` and the
            // click position in `wparam`.
            let event = (lparam.0 as u32) & 0xFFFF;
            let at = POINT {
                x: (wparam.0 as u32 & 0xFFFF) as i16 as i32,
                y: ((wparam.0 as u32 >> 16) & 0xFFFF) as i16 as i32,
            };
            match event {
                // One click sends `WM_LBUTTONUP` and then `NIN_SELECT`.
                // Toggling on both shows the window and hides it again 5 ms
                // later, so only the select counts. `NIN_KEYSELECT` is the
                // keyboard's Enter or Space on the icon.
                NIN_SELECT | NIN_KEYSELECT => {
                    app.toggle();
                }
                WM_CONTEXTMENU | WM_RBUTTONUP => app.show_menu(at),
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER => {
            match wparam.0 {
                TIMER_POLL => app.poll(),
                TIMER_TRAY_RETRY => app.add_tray_icon(),
                _ => {}
            }
            LRESULT(0)
        }
        WM_PAINT => {
            app.paint();
            LRESULT(0)
        }
        // The renderer copies a full buffer, so erasing would only flicker.
        WM_ERASEBKGND => LRESULT(1),
        WM_LBUTTONDOWN => {
            let point = mouse_point(lparam);
            // The header is the drag handle. The minimize button is not.
            let on_button = contains(&app.layout.minimize_rect, point);
            if point.y < app.layout.header_bottom && !on_button {
                app.dragging = true;
                unsafe {
                    let _ = PostMessageW(
                        Some(window),
                        WM_NCLBUTTONDOWN,
                        WPARAM(HTCAPTION as usize),
                        lparam,
                    );
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            app.on_click(mouse_point(lparam));
            LRESULT(0)
        }
        WM_EXITSIZEMOVE => {
            app.store_position();
            LRESULT(0)
        }
        WM_CONTEXTMENU => {
            let at = POINT {
                x: (lparam.0 as u32 & 0xFFFF) as i16 as i32,
                y: ((lparam.0 as u32 >> 16) & 0xFFFF) as i16 as i32,
            };
            app.show_menu(at);
            LRESULT(0)
        }
        WM_COMMAND => {
            app.on_command(wparam.0 & 0xFFFF);
            LRESULT(0)
        }
        WM_NOTIFY => {
            let header = unsafe { &mut *(lparam.0 as *mut NMTTDISPINFOW) };
            if header.hdr.code == TTN_GETDISPINFOW {
                let text = "Minimize to tray";
                for (slot, unit) in header.szText.iter_mut().zip(text.encode_utf16().chain([0])) {
                    *slot = unit;
                }
                header.lpszText = PWSTR(header.szText.as_mut_ptr());
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            // `lparam` points at the rectangle Windows suggests for the new scale.
            let suggested = unsafe { *(lparam.0 as *const RECT) };
            app.on_dpi_changed((wparam.0 as u32) & 0xFFFF, suggested);
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            if app.visible {
                app.place();
            }
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            // PDH state does not survive sleep. Rebuild the whole query.
            let event = wparam.0 as u32;
            if event == PBT_APMRESUMEAUTOMATIC || event == PBT_APMRESUMESUSPEND {
                let _ = app.sampler.rebuild();
            }
            LRESULT(1)
        }
        WM_DESTROY => {
            unsafe {
                let _ = KillTimer(Some(window), TIMER_POLL);
                let _ = KillTimer(Some(window), TIMER_TRAY_RETRY);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

fn mouse_point(lparam: LPARAM) -> POINT {
    POINT {
        x: (lparam.0 as u32 & 0xFFFF) as i16 as i32,
        y: ((lparam.0 as u32 >> 16) & 0xFFFF) as i16 as i32,
    }
}

/// Move `y` up just enough that a window `height` tall ends above the bottom
/// of `work`, but never above its top.
fn keep_bottom_visible(y: i32, height: i32, work: Option<RECT>) -> i32 {
    match work {
        Some(work) if y + height > work.bottom => (work.bottom - height).max(work.top),
        _ => y,
    }
}

/// The effective DPI of a monitor.
fn monitor_dpi(monitor: HMONITOR) -> Option<u32> {
    let (mut x, mut y) = (0, 0);
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) }.ok()?;
    Some(x.max(96))
}

/// The work area of a monitor, which excludes the taskbar.
fn work_area(monitor: HMONITOR) -> Option<RECT> {
    use windows::Win32::Graphics::Gdi::GetMonitorInfoW;

    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        Some(info.rcWork)
    } else {
        None
    }
}

fn write_tip(buffer: &mut [u16; 128], text: &str) {
    buffer.fill(0);
    for (slot, unit) in buffer
        .iter_mut()
        .take(127)
        .zip(text.encode_utf16())
    {
        *slot = unit;
    }
}

// --------------------------------------------------------- the tray icon ----

/// Draw the tray icon at runtime, so the build needs no resource file.
///
/// Four bars of rising height. The pixels go straight into a 32-bit DIB, so
/// the alpha channel is exact and no font or shape call is involved.
fn make_tray_icon() -> HICON {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64);

    let mut header = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            // Negative height makes the rows top down, which is easier to
            // reason about than the default bottom-up order.
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut pixels: *mut core::ffi::c_void = std::ptr::null_mut();
    let screen = unsafe { GetDC(None) };
    let color = unsafe {
        CreateDIBSection(
            Some(screen),
            &raw const header,
            DIB_RGB_COLORS,
            &raw mut pixels,
            None,
            0,
        )
    };
    unsafe { ReleaseDC(None, screen) };
    let _ = &mut header;

    let Ok(color) = color else {
        return HICON::default();
    };
    if pixels.is_null() {
        return HICON::default();
    }

    // The accent blue reads on a light taskbar and on a dark one.
    const BAR: (u8, u8, u8) = (0x4C, 0x9A, 0xFF);
    let count = 4i32;
    let gap = (size / 12).max(1);
    let bar_width = ((size - gap * (count - 1)) / count).max(1);
    let left_over = size - (bar_width * count + gap * (count - 1));
    let start = left_over / 2;

    let row = size as usize;
    let buffer = unsafe { std::slice::from_raw_parts_mut(pixels.cast::<u32>(), row * row) };
    buffer.fill(0);

    for index in 0..count {
        let height = (size * (index + 2)) / (count + 1);
        let x0 = start + index * (bar_width + gap);
        for y in (size - height)..size {
            for x in x0..(x0 + bar_width).min(size) {
                // Premultiplied BGRA, with alpha at full.
                buffer[y as usize * row + x as usize] = 0xFF00_0000
                    | ((BAR.0 as u32) << 16)
                    | ((BAR.1 as u32) << 8)
                    | (BAR.2 as u32);
            }
        }
    }

    // A 1-bit mask of zeros means "use the alpha channel".
    let mask: HBITMAP = unsafe { CreateBitmap(size, size, 1, 1, None) };
    let info = ICONINFO {
        fIcon: true.into(),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color,
    };
    let icon = unsafe { CreateIconIndirect(&info) };
    unsafe {
        use windows::Win32::Graphics::Gdi::DeleteObject;
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
    }
    icon.unwrap_or_default()
}

// --------------------------------------------- start with Windows ----------

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const RUN_VALUE: PCWSTR = w!("Glint");

fn startup_enabled() -> bool {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

    let mut size = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    status.is_ok() && size > 0
}

fn set_startup_enabled(enable: bool) {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_SET_VALUE, REG_SZ,
    };

    let mut key = HKEY::default();
    let opened = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key) };
    if opened.is_err() {
        return;
    }
    if enable {
        // The path is quoted, so a space in it cannot split the command.
        let Ok(exe) = std::env::current_exe() else {
            unsafe {
                let _ = RegCloseKey(key);
            }
            return;
        };
        let quoted = format!("\"{}\"", exe.display());
        let units: Vec<u16> = quoted.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = unsafe {
            std::slice::from_raw_parts(units.as_ptr().cast::<u8>(), units.len() * 2)
        };
        unsafe {
            let _ = RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(bytes));
        }
    } else {
        unsafe {
            let _ = RegDeleteValueW(key, RUN_VALUE);
        }
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
}
