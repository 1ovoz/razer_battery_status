//! System32에서만 불러오는 DLL 함수들.
//! KnownDLLs 목록에 없는 DLL을 정적으로 가져오면, exe와 같은 폴더(예: 다운로드 폴더)에
//! 같은 이름의 가짜 DLL이 있을 때 그걸 먼저 불러올 수 있다(DLL 바꿔치기).
//! 그래서 이 세 DLL은 처음 쓸 때 System32에서 직접 불러온다.

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{ERROR_DEVICE_NOT_CONNECTED, FARPROC, HWND};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
use windows_sys::Win32::UI::Input::XboxController::{BATTERY_DEVTYPE_GAMEPAD, XINPUT_BATTERY_INFORMATION};

use crate::tray::wide;

fn system_proc(dll: &str, name: &'static [u8]) -> FARPROC {
    unsafe {
        let h = LoadLibraryExW(wide(dll).as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
        if h.is_null() {
            return None;
        }
        GetProcAddress(h, name.as_ptr())
    }
}

type XInputBatteryFn = unsafe extern "system" fn(u32, u8, *mut XINPUT_BATTERY_INFORMATION) -> u32;
type SetWindowThemeFn = unsafe extern "system" fn(HWND, *const u16, *const u16) -> i32;
type DwmSetAttrFn = unsafe extern "system" fn(HWND, u32, *const c_void, u32) -> i32;

/// XInputGetBatteryInformation (xinput1_4.dll). DLL을 못 불러오면 "연결 안 됨".
pub fn xinput_battery(user: u32, info: &mut XINPUT_BATTERY_INFORMATION) -> u32 {
    static F: OnceLock<Option<XInputBatteryFn>> = OnceLock::new();
    let f = F.get_or_init(|| {
        system_proc("xinput1_4.dll", b"XInputGetBatteryInformation\0")
            .map(|p| unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, XInputBatteryFn>(p) })
    });
    match f {
        Some(f) => unsafe { f(user, BATTERY_DEVTYPE_GAMEPAD, info) },
        None => ERROR_DEVICE_NOT_CONNECTED,
    }
}

/// SetWindowTheme (uxtheme.dll) — 설정 창 컨트롤 어두운 테마
pub fn set_window_theme(hwnd: HWND, app_name: &str) {
    static F: OnceLock<Option<SetWindowThemeFn>> = OnceLock::new();
    let f = F.get_or_init(|| {
        system_proc("uxtheme.dll", b"SetWindowTheme\0")
            .map(|p| unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, SetWindowThemeFn>(p) })
    });
    if let Some(f) = f {
        unsafe { f(hwnd, wide(app_name).as_ptr(), std::ptr::null()) };
    }
}

/// DwmSetWindowAttribute (dwmapi.dll) — 어두운 제목 표시줄
pub fn dwm_set_u32(hwnd: HWND, attr: u32, value: u32) {
    static F: OnceLock<Option<DwmSetAttrFn>> = OnceLock::new();
    let f = F.get_or_init(|| {
        system_proc("dwmapi.dll", b"DwmSetWindowAttribute\0")
            .map(|p| unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, DwmSetAttrFn>(p) })
    });
    if let Some(f) = f {
        unsafe { f(hwnd, attr, &value as *const u32 as *const c_void, 4) };
    }
}
