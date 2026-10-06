//! glint: a tray system monitor for Windows.
//!
//! `windows_subsystem = "windows"` stops the console window from appearing.
//! Use the `sample` or `spike` binaries for console output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

/// Hold the single-instance mutex for as long as the app runs.
struct SingleInstance(HANDLE);

impl Drop for SingleInstance {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// Claim the one-instance name. `None` means another instance already runs.
///
/// The name is per session (`Local\`), so two signed-in users each get their
/// own tray icon.
fn claim_single_instance() -> Option<SingleInstance> {
    let handle = unsafe { CreateMutexW(None, true, w!(r"Local\glint.instance")) };
    let Ok(handle) = handle else {
        return None;
    };
    // `CreateMutexW` succeeds either way, so the last error is the answer.
    if unsafe { windows::Win32::Foundation::GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            let _ = CloseHandle(handle);
        }
        return None;
    }
    Some(SingleInstance(handle))
}

fn main() {
    let Some(_instance) = claim_single_instance() else {
        // A second launch is not an error. The first instance owns the icon.
        return;
    };
    if let Err(error) = glint::window::run() {
        report(&format!("glint could not start.\n\n{error}"));
    }
}

/// Show a message box. A windows-subsystem binary has no console to print to.
fn report(text: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let body: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            w!("Glint"),
            MB_OK | MB_ICONERROR,
        );
    }
}
