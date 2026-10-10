//! 설정 창 (Win32 기본 컨트롤). 트레이 메뉴나 위젯 톱니바퀴에서 연다.
//! 자동 실행은 HKCU\...\Run 레지스트리 값으로 켜고 끈다 (관리자 권한 불필요).

use std::cell::RefCell;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicPtr, Ordering};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use razer_battery_tray::config::{Config, Remind, DEVICES};

use crate::tray::{wide, with_app};

static HWND_SETTINGS: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(null_mut());

/// 메시지 루프에서 IsDialogMessage(Tab 이동, Enter=저장)에 쓰는 설정 창 핸들
pub fn hwnd() -> HWND {
    HWND_SETTINGS.load(Ordering::Relaxed)
}

thread_local! {
    /// 창을 열 때의 설정 (저장 시 위치·배치처럼 이 창에 없는 값을 보존)
    static BASE: RefCell<Option<(Config, [String; DEVICES])>> = const { RefCell::new(None) };
    static FONT: RefCell<HFONT> = const { RefCell::new(null_mut()) };
}

const ID_AUTOSTART: i32 = 100;
const ID_WIDGET: i32 = 200; // +i
const ID_TRAY: i32 = 210; // +i
const ID_NICK: i32 = 220; // +i
const ID_THRESHOLD: i32 = 300;
const ID_REMIND: i32 = 301;
const ID_OPACITY: i32 = 302;
const ID_OPACITY_LABEL: i32 = 303;

// windows-sys에서 다른 기능 묶음에 있는 상수들
const SS_CENTER: u32 = 0x1;
const SS_CENTERIMAGE: u32 = 0x200;
const TBM_GETPOS: u32 = WM_USER;

const THRESHOLDS: [u8; 10] = [5, 10, 15, 20, 25, 30, 35, 40, 45, 50];

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "RazerBatteryTray";

pub fn autostart_enabled() -> bool {
    let (k, v) = (wide(RUN_KEY), wide(RUN_VALUE));
    let mut size = 0u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), RRF_RT_REG_SZ, null_mut(), null_mut(), &mut size)
            == ERROR_SUCCESS
    }
}

pub fn set_autostart(on: bool) {
    let (k, v) = (wide(RUN_KEY), wide(RUN_VALUE));
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return };
            let data = wide(&format!("\"{}\"", exe.display()));
            RegSetKeyValueW(
                HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), REG_SZ,
                data.as_ptr() as *const _, (data.len() * 2) as u32,
            );
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr());
        }
    }
}

/// 설정 창 열기 (이미 열려 있으면 앞으로 가져옴). names = 장치 기본 이름
pub fn open(config: Config, names: [String; DEVICES]) {
    unsafe {
        let existing = hwnd();
        if !existing.is_null() {
            SetForegroundWindow(existing);
            return;
        }
        let icc = INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_BAR_CLASSES };
        InitCommonControlsEx(&icc);

        let hinst = GetModuleHandleW(null());
        let class = wide("RazerBatterySettings");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_BTNFACE + 1) as usize as HBRUSH,
            ..zeroed()
        };
        RegisterClassW(&wc);

        let dpi = GetDpiForSystem();
        let s = |v: i32| v * dpi as i32 / 96;
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut r = RECT { left: 0, top: 0, right: s(570), bottom: s(370) };
        AdjustWindowRectExForDpi(&mut r, style, 0, 0, dpi);
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let x = (GetSystemMetrics(SM_CXSCREEN) - w) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - h) / 2;

        BASE.with(|b| *b.borrow_mut() = Some((config, names)));
        let hwnd = CreateWindowExW(
            0, class.as_ptr(), wide("Razer 배터리 설정").as_ptr(), style,
            x, y, w, h, null_mut(), null_mut(), hinst, null(),
        );
        if hwnd.is_null() {
            return;
        }
        HWND_SETTINGS.store(hwnd, Ordering::Relaxed);
        build_controls(hwnd, dpi);
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetForegroundWindow(hwnd);
    }
}

unsafe fn control(parent: HWND, class: &str, text: &str, style: u32, x: i32, y: i32, w: i32, h: i32, id: i32) -> HWND {
    unsafe {
        let hwnd = CreateWindowExW(
            0, wide(class).as_ptr(), wide(text).as_ptr(), WS_CHILD | WS_VISIBLE | style,
            x, y, w, h, parent, id as usize as HMENU, GetModuleHandleW(null()), null(),
        );
        FONT.with(|f| SendMessageW(hwnd, WM_SETFONT, *f.borrow() as WPARAM, 1));
        hwnd
    }
}

unsafe fn build_controls(hwnd: HWND, dpi: u32) {
    unsafe {
        let s = |v: i32| v * dpi as i32 / 96;
        let font = CreateFontW(
            -s(13), 0, 0, 0, FW_NORMAL as i32, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32, CLEARTYPE_QUALITY as u32, DEFAULT_PITCH as u32, wide("Malgun Gothic").as_ptr(),
        );
        FONT.with(|f| *f.borrow_mut() = font);
        let Some((cfg, names)) = BASE.with(|b| b.borrow().clone()) else { return };
        let check = |h: HWND, on: bool| SendMessageW(h, BM_SETCHECK, if on { BST_CHECKED } else { BST_UNCHECKED } as WPARAM, 0);
        let cb = (BS_AUTOCHECKBOX | WS_TABSTOP as i32) as u32;

        let h = control(hwnd, "BUTTON", "Windows 시작 시 자동 실행", cb, s(16), s(12), s(400), s(24), ID_AUTOSTART);
        check(h, autostart_enabled());

        control(hwnd, "STATIC", "장치", 0, s(16), s(48), s(170), s(20), -1);
        control(hwnd, "STATIC", "위젯", SS_CENTER, s(186), s(48), s(56), s(20), -1);
        control(hwnd, "STATIC", "작업표시줄", SS_CENTER, s(244), s(48), s(80), s(20), -1);
        control(hwnd, "STATIC", "표시 이름 (비우면 기본 이름)", 0, s(334), s(48), s(230), s(20), -1);
        for i in 0..DEVICES {
            let y = s(74 + i as i32 * 32);
            control(hwnd, "STATIC", &names[i], SS_CENTERIMAGE, s(16), y, s(170), s(26), -1);
            let h = control(hwnd, "BUTTON", "", cb, s(206), y, s(20), s(26), ID_WIDGET + i as i32);
            check(h, cfg.show_widget[i]);
            let h = control(hwnd, "BUTTON", "", cb, s(276), y, s(20), s(26), ID_TRAY + i as i32);
            check(h, cfg.show_tray[i]);
            let h = control(
                hwnd, "EDIT", &cfg.nickname[i], (WS_BORDER | WS_TABSTOP) | ES_AUTOHSCROLL as u32,
                s(334), y, s(220), s(26), ID_NICK + i as i32,
            );
            SendMessageW(h, EM_LIMITTEXT, 40, 0);
        }

        let combo_style = (WS_TABSTOP | WS_VSCROLL) | CBS_DROPDOWNLIST as u32;
        control(hwnd, "STATIC", "배터리 부족 기준", SS_CENTERIMAGE, s(16), s(212), s(170), s(26), -1);
        let h = control(hwnd, "COMBOBOX", "", combo_style, s(206), s(212), s(130), s(300), ID_THRESHOLD);
        for (i, t) in THRESHOLDS.iter().enumerate() {
            SendMessageW(h, CB_ADDSTRING, 0, wide(&format!("{t}% 미만")).as_ptr() as LPARAM);
            if *t == cfg.low_threshold {
                SendMessageW(h, CB_SETCURSEL, i, 0);
            }
        }
        if SendMessageW(h, CB_GETCURSEL, 0, 0) < 0 {
            SendMessageW(h, CB_SETCURSEL, 2, 0); // 15%
        }

        control(hwnd, "STATIC", "부족 알림", SS_CENTERIMAGE, s(16), s(248), s(170), s(26), -1);
        let h = control(hwnd, "COMBOBOX", "", combo_style, s(206), s(248), s(130), s(300), ID_REMIND);
        for (i, r) in Remind::CHOICES.iter().enumerate() {
            SendMessageW(h, CB_ADDSTRING, 0, wide(&r.label()).as_ptr() as LPARAM);
            if *r == cfg.remind {
                SendMessageW(h, CB_SETCURSEL, i, 0);
            }
        }
        if SendMessageW(h, CB_GETCURSEL, 0, 0) < 0 {
            SendMessageW(h, CB_SETCURSEL, 5, 0); // 1번만
        }

        control(hwnd, "STATIC", "위젯 배경 불투명도", SS_CENTERIMAGE, s(16), s(284), s(170), s(28), -1);
        let h = control(hwnd, "msctls_trackbar32", "", WS_TABSTOP | TBS_NOTICKS as u32, s(200), s(284), s(290), s(30), ID_OPACITY);
        SendMessageW(h, TBM_SETRANGE, 1, (100 << 16) as LPARAM);
        SendMessageW(h, TBM_SETPOS, 1, cfg.bg_opacity as LPARAM);
        control(hwnd, "STATIC", &format!("{}%", cfg.bg_opacity), SS_CENTERIMAGE, s(496), s(284), s(60), s(28), ID_OPACITY_LABEL);

        let btn = WS_TABSTOP;
        control(hwnd, "BUTTON", "저장", btn | BS_DEFPUSHBUTTON as u32, s(344), s(328), s(100), s(30), IDOK);
        control(hwnd, "BUTTON", "취소", btn, s(454), s(328), s(100), s(30), IDCANCEL);
    }
}

unsafe fn item(hwnd: HWND, id: i32) -> HWND {
    unsafe { GetDlgItem(hwnd, id) }
}

unsafe fn checked(hwnd: HWND, id: i32) -> bool {
    unsafe { SendMessageW(item(hwnd, id), BM_GETCHECK, 0, 0) == BST_CHECKED as LRESULT }
}

unsafe fn text(hwnd: HWND, id: i32) -> String {
    unsafe {
        let mut buf = [0u16; 64];
        let n = GetWindowTextW(item(hwnd, id), buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

unsafe fn save(hwnd: HWND) {
    unsafe {
        // 위젯에서 바꾼 배치·크기·위치 등은 지금 값을 유지하고, 이 창의 항목만 덮어쓴다
        let Some(mut cfg) = with_app(|a| a.config.clone()) else { return };
        for i in 0..DEVICES {
            cfg.show_widget[i] = checked(hwnd, ID_WIDGET + i as i32);
            cfg.show_tray[i] = checked(hwnd, ID_TRAY + i as i32);
            cfg.nickname[i] = text(hwnd, ID_NICK + i as i32).trim().to_string();
        }
        let t = SendMessageW(item(hwnd, ID_THRESHOLD), CB_GETCURSEL, 0, 0);
        cfg.low_threshold = THRESHOLDS.get(t as usize).copied().unwrap_or(15);
        let r = SendMessageW(item(hwnd, ID_REMIND), CB_GETCURSEL, 0, 0);
        cfg.remind = Remind::CHOICES.get(r as usize).copied().unwrap_or(Remind::Once);
        cfg.bg_opacity = SendMessageW(item(hwnd, ID_OPACITY), TBM_GETPOS, 0, 0).clamp(0, 100) as u8;

        let want = checked(hwnd, ID_AUTOSTART);
        if want != autostart_enabled() {
            set_autostart(want);
        }
        with_app(|a| a.apply_config(cfg));
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND => {
                match (wp & 0xFFFF) as i32 {
                    IDOK => {
                        save(hwnd);
                        DestroyWindow(hwnd);
                    }
                    IDCANCEL => {
                        DestroyWindow(hwnd);
                    }
                    _ => {}
                }
                0
            }
            WM_HSCROLL => {
                let v = SendMessageW(item(hwnd, ID_OPACITY), TBM_GETPOS, 0, 0);
                SetWindowTextW(item(hwnd, ID_OPACITY_LABEL), wide(&format!("{v}%")).as_ptr());
                0
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                HWND_SETTINGS.store(null_mut(), Ordering::Relaxed);
                FONT.with(|f| {
                    DeleteObject(*f.borrow());
                    *f.borrow_mut() = null_mut();
                });
                BASE.with(|b| *b.borrow_mut() = None);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
