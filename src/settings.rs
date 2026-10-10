//! 설정 창. 트레이 메뉴나 위젯 톱니바퀴에서 연다.
//! Win32 기본 컨트롤에 어두운 테마를 입히고, 위젯처럼 둥근 카드 위에 배치한다.
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
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use razer_battery_tray::config::{Config, Remind, DEVICES, POLL_CHOICES, POLL_DEFAULT};

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
    /// 행 → 장치 번호 (▲▼로 바꾸는 타일 순서)
    static ORDER: RefCell<[usize; DEVICES]> = RefCell::new(std::array::from_fn(|i| i));
    /// 배경·카드·입력칸 브러시
    static BRUSHES: RefCell<Option<(HBRUSH, HBRUSH, HBRUSH)>> = const { RefCell::new(None) };
    /// 카드 사각형 (96 DPI 기준 좌표를 DPI에 맞춘 값)
    static CARDS: RefCell<Vec<RECT>> = const { RefCell::new(Vec::new()) };
}

// 색 (위젯과 같은 계열)
const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}
const C_WINDOW: COLORREF = rgb(16, 16, 18);
const C_CARD: COLORREF = rgb(30, 30, 34);
const C_EDIT: COLORREF = rgb(44, 44, 50);
const C_TEXT: COLORREF = rgb(236, 236, 240);
const C_SUBTEXT: COLORREF = rgb(150, 150, 155);

const ID_AUTOSTART: i32 = 100;
const ID_UPDATE_CHECK: i32 = 101;
const ID_WIDGET: i32 = 200; // +i
const ID_TRAY: i32 = 210; // +i
const ID_NICK: i32 = 220; // +i
const ID_THRESHOLD: i32 = 300;
const ID_REMIND: i32 = 301;
const ID_OPACITY: i32 = 302;
const ID_OPACITY_LABEL: i32 = 303;
const ID_POLL: i32 = 304;
const ID_NAME: i32 = 230; // +행: 장치 이름 글자
const ID_UP: i32 = 240; // +행
const ID_DOWN: i32 = 250; // +행
const ID_HEADER: i32 = 500; // 이 번호 이상인 글자는 흐린 색
const ID_VERSION: i32 = 600; // 창 배경 위의 버전 글자

// windows-sys에서 다른 기능 묶음에 있는 상수들
const SS_CENTER: u32 = 0x1;
const SS_CENTERIMAGE: u32 = 0x200;
const TBM_GETPOS: u32 = WM_USER;

/// 장치가 4개보다 많거나 적을 때 장치 카드 아래쪽을 밀거나 당기는 양 (96 DPI 기준)
const DY: i32 = (DEVICES as i32 - 4) * 42;

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
            ..zeroed()
        };
        RegisterClassW(&wc);

        let dpi = GetDpiForSystem();
        let s = |v: i32| v * dpi as i32 / 96;
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut r = RECT { left: 0, top: 0, right: s(600), bottom: s(516 + DY) };
        AdjustWindowRectExForDpi(&mut r, style, 0, 0, dpi);
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let x = (GetSystemMetrics(SM_CXSCREEN) - w) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - h) / 2;

        ORDER.with(|o| *o.borrow_mut() = config.order);
        BRUSHES.with(|b| *b.borrow_mut() = Some((CreateSolidBrush(C_WINDOW), CreateSolidBrush(C_CARD), CreateSolidBrush(C_EDIT))));
        BASE.with(|b| *b.borrow_mut() = Some((config, names)));
        let hwnd = CreateWindowExW(
            0, class.as_ptr(), wide("Razer 배터리 설정").as_ptr(), style,
            x, y, w, h, null_mut(), null_mut(), hinst, null(),
        );
        if hwnd.is_null() {
            return;
        }
        HWND_SETTINGS.store(hwnd, Ordering::Relaxed);
        // 어두운 제목 표시줄 (Windows 10 20H1+), 제목 표시줄 색을 배경색과 맞춤 (Windows 11)
        crate::syslib::dwm_set_u32(hwnd, 20, 1);
        crate::syslib::dwm_set_u32(hwnd, 35, C_WINDOW);
        build_controls(hwnd, dpi);
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetForegroundWindow(hwnd);
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn control(parent: HWND, class: &str, text: &str, style: u32, x: i32, y: i32, w: i32, h: i32, id: i32) -> HWND {
    unsafe {
        let hwnd = CreateWindowExW(
            0, wide(class).as_ptr(), wide(text).as_ptr(), WS_CHILD | WS_VISIBLE | style,
            x, y, w, h, parent, id as usize as HMENU, GetModuleHandleW(null()), null(),
        );
        FONT.with(|f| SendMessageW(hwnd, WM_SETFONT, *f.borrow() as WPARAM, 1));
        // 어두운 테마 (콤보·입력칸은 CFD, 버튼·체크박스는 Explorer)
        let theme = match class {
            "COMBOBOX" | "EDIT" => Some("DarkMode_CFD"),
            "BUTTON" => Some("DarkMode_Explorer"),
            _ => None,
        };
        if let Some(t) = theme {
            crate::syslib::set_window_theme(hwnd, t);
        }
        hwnd
    }
}

/// 장치 한 행의 입력값
#[derive(Clone, Default)]
struct Row {
    widget: bool,
    tray: bool,
    nick: String,
}

/// 화면의 행들을 장치 번호 순으로 읽는다
unsafe fn read_rows(hwnd: HWND) -> [Row; DEVICES] {
    unsafe {
        let order = ORDER.with(|o| *o.borrow());
        let mut rows: [Row; DEVICES] = Default::default();
        for (r, &d) in order.iter().enumerate() {
            let r = r as i32;
            rows[d] = Row {
                widget: checked(hwnd, ID_WIDGET + r),
                tray: checked(hwnd, ID_TRAY + r),
                nick: text(hwnd, ID_NICK + r),
            };
        }
        rows
    }
}

/// 장치별 입력값을 현재 순서대로 화면 행에 쓴다
unsafe fn write_rows(hwnd: HWND, rows: &[Row; DEVICES]) {
    unsafe {
        let order = ORDER.with(|o| *o.borrow());
        let names = BASE.with(|b| b.borrow().as_ref().map(|(_, n)| n.clone())).unwrap_or_default();
        let check = |id: i32, on: bool| {
            SendMessageW(item(hwnd, id), BM_SETCHECK, if on { BST_CHECKED } else { BST_UNCHECKED } as WPARAM, 0);
        };
        for (r, &d) in order.iter().enumerate() {
            let r = r as i32;
            SetWindowTextW(item(hwnd, ID_NAME + r), wide(&names[d]).as_ptr());
            check(ID_WIDGET + r, rows[d].widget);
            check(ID_TRAY + r, rows[d].tray);
            SetWindowTextW(item(hwnd, ID_NICK + r), wide(&rows[d].nick).as_ptr());
            EnableWindow(item(hwnd, ID_UP + r), (r > 0) as i32);
            EnableWindow(item(hwnd, ID_DOWN + r), (r < DEVICES as i32 - 1) as i32);
        }
    }
}

/// 행 r을 위(-1)/아래(+1)로 옮긴다
unsafe fn move_row(hwnd: HWND, r: usize, dir: isize) {
    unsafe {
        let to = r as isize + dir;
        if to < 0 || to >= DEVICES as isize {
            return;
        }
        let rows = read_rows(hwnd);
        ORDER.with(|o| o.borrow_mut().swap(r, to as usize));
        write_rows(hwnd, &rows);
        SetFocus(item(hwnd, if dir < 0 { ID_UP } else { ID_DOWN } + to as i32));
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
        let Some((cfg, _)) = BASE.with(|b| b.borrow().clone()) else { return };
        let check = |h: HWND, on: bool| SendMessageW(h, BM_SETCHECK, if on { BST_CHECKED } else { BST_UNCHECKED } as WPARAM, 0);
        let cb = (BS_AUTOCHECKBOX | WS_TABSTOP as i32) as u32;
        let label = |text: &str, x: i32, y: i32, w: i32, id: i32| {
            control(hwnd, "STATIC", text, SS_CENTERIMAGE, s(x), s(y), s(w), s(28), id)
        };

        // 카드 1: 일반
        CARDS.with(|c| {
            *c.borrow_mut() = vec![
                RECT { left: s(12), top: s(12), right: s(588), bottom: s(60) },
                RECT { left: s(12), top: s(70), right: s(588), bottom: s(284 + DY) },
                RECT { left: s(12), top: s(294 + DY), right: s(588), bottom: s(460 + DY) },
            ]
        });
        let h = control(hwnd, "BUTTON", "", cb, s(28), s(22), s(20), s(28), ID_AUTOSTART);
        check(h, autostart_enabled());
        label("Windows 시작 시 자동 실행", 54, 22, 230, -1);
        let h = control(hwnd, "BUTTON", "", cb, s(310), s(22), s(20), s(28), ID_UPDATE_CHECK);
        check(h, cfg.update_check);
        label("새 버전 알림 (하루 한 번 확인)", 336, 22, 240, -1);

        // 카드 2: 장치 (순서 ▲▼, 이름, 위젯, 작업표시줄, 표시 이름)
        label("순서", 28, 80, 50, ID_HEADER);
        label("장치", 88, 80, 160, ID_HEADER + 1);
        control(hwnd, "STATIC", "위젯", SS_CENTER | SS_CENTERIMAGE, s(256), s(80), s(56), s(28), ID_HEADER + 2);
        control(hwnd, "STATIC", "작업표시줄", SS_CENTER | SS_CENTERIMAGE, s(316), s(80), s(84), s(28), ID_HEADER + 3);
        label("표시 이름 (닉네임)", 410, 80, 170, ID_HEADER + 4);
        for r in 0..DEVICES as i32 {
            let y = 112 + r * 42;
            control(hwnd, "BUTTON", "▲", WS_TABSTOP, s(28), s(y), s(26), s(28), ID_UP + r);
            control(hwnd, "BUTTON", "▼", WS_TABSTOP, s(56), s(y), s(26), s(28), ID_DOWN + r);
            label("", 88, y, 166, ID_NAME + r);
            control(hwnd, "BUTTON", "", cb, s(276), s(y), s(20), s(28), ID_WIDGET + r);
            control(hwnd, "BUTTON", "", cb, s(350), s(y), s(20), s(28), ID_TRAY + r);
            let h = control(
                hwnd, "EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL as u32,
                s(410), s(y + 2), s(166), s(24), ID_NICK + r,
            );
            SendMessageW(h, EM_LIMITTEXT, 40, 0);
        }
        let rows: [Row; DEVICES] = std::array::from_fn(|d| Row {
            widget: cfg.show_widget[d],
            tray: cfg.show_tray[d],
            nick: cfg.nickname[d].clone(),
        });
        write_rows(hwnd, &rows);

        // 카드 3: 알림·위젯 모양
        let combo_style = (WS_TABSTOP | WS_VSCROLL) | CBS_DROPDOWNLIST as u32;
        label("배터리 부족 기준", 28, 306 + DY, 180, -1);
        let h = control(hwnd, "COMBOBOX", "", combo_style, s(220), s(306 + DY), s(150), s(300), ID_THRESHOLD);
        for (i, t) in THRESHOLDS.iter().enumerate() {
            SendMessageW(h, CB_ADDSTRING, 0, wide(&format!("{t}% 미만")).as_ptr() as LPARAM);
            if *t == cfg.low_threshold {
                SendMessageW(h, CB_SETCURSEL, i, 0);
            }
        }
        if SendMessageW(h, CB_GETCURSEL, 0, 0) < 0 {
            SendMessageW(h, CB_SETCURSEL, 2, 0); // 15%
        }

        label("부족 알림", 28, 344 + DY, 180, -1);
        let h = control(hwnd, "COMBOBOX", "", combo_style, s(220), s(344 + DY), s(150), s(300), ID_REMIND);
        for (i, r) in Remind::CHOICES.iter().enumerate() {
            SendMessageW(h, CB_ADDSTRING, 0, wide(&r.label()).as_ptr() as LPARAM);
            if *r == cfg.remind {
                SendMessageW(h, CB_SETCURSEL, i, 0);
            }
        }
        if SendMessageW(h, CB_GETCURSEL, 0, 0) < 0 {
            SendMessageW(h, CB_SETCURSEL, 5, 0); // 1번만
        }

        label("갱신 주기", 28, 382 + DY, 180, -1);
        let h = control(hwnd, "COMBOBOX", "", combo_style, s(220), s(382 + DY), s(150), s(300), ID_POLL);
        for (i, p) in POLL_CHOICES.iter().enumerate() {
            let text = if *p == POLL_DEFAULT { format!("{p}초 (추천)") } else { format!("{p}초") };
            SendMessageW(h, CB_ADDSTRING, 0, wide(&text).as_ptr() as LPARAM);
            if *p == cfg.poll_secs {
                SendMessageW(h, CB_SETCURSEL, i, 0);
            }
        }
        if SendMessageW(h, CB_GETCURSEL, 0, 0) < 0 {
            SendMessageW(h, CB_SETCURSEL, 1, 0); // 5초
        }

        label("위젯 배경 불투명도", 28, 420 + DY, 180, -1);
        let h = control(hwnd, "msctls_trackbar32", "", WS_TABSTOP | TBS_NOTICKS, s(214), s(420 + DY), s(300), s(28), ID_OPACITY);
        SendMessageW(h, TBM_SETRANGE, 1, (100 << 16) as LPARAM);
        SendMessageW(h, TBM_SETPOS, 1, cfg.bg_opacity as LPARAM);
        label(&format!("{}%", cfg.bg_opacity), 520, 420 + DY, 60, ID_OPACITY_LABEL);

        label(&format!("버전 {}", env!("CARGO_PKG_VERSION")), 16, 476 + DY, 200, ID_VERSION);

        control(hwnd, "BUTTON", "저장", WS_TABSTOP | BS_DEFPUSHBUTTON as u32, s(372), s(474 + DY), s(100), s(32), IDOK);
        control(hwnd, "BUTTON", "취소", WS_TABSTOP, s(482), s(474 + DY), s(100), s(32), IDCANCEL);
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
        let rows = read_rows(hwnd);
        for (d, row) in rows.iter().enumerate() {
            cfg.show_widget[d] = row.widget;
            cfg.show_tray[d] = row.tray;
            cfg.nickname[d] = row.nick.trim().to_string();
        }
        cfg.order = ORDER.with(|o| *o.borrow());
        let t = SendMessageW(item(hwnd, ID_THRESHOLD), CB_GETCURSEL, 0, 0);
        cfg.low_threshold = THRESHOLDS.get(t as usize).copied().unwrap_or(15);
        let r = SendMessageW(item(hwnd, ID_REMIND), CB_GETCURSEL, 0, 0);
        cfg.remind = Remind::CHOICES.get(r as usize).copied().unwrap_or(Remind::Once);
        cfg.bg_opacity = SendMessageW(item(hwnd, ID_OPACITY), TBM_GETPOS, 0, 0).clamp(0, 100) as u8;
        let p = SendMessageW(item(hwnd, ID_POLL), CB_GETCURSEL, 0, 0);
        cfg.poll_secs = POLL_CHOICES.get(p as usize).copied().unwrap_or(POLL_DEFAULT);

        cfg.update_check = checked(hwnd, ID_UPDATE_CHECK);
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
                    id if (ID_UP..ID_UP + DEVICES as i32).contains(&id) => move_row(hwnd, (id - ID_UP) as usize, -1),
                    id if (ID_DOWN..ID_DOWN + DEVICES as i32).contains(&id) => move_row(hwnd, (id - ID_DOWN) as usize, 1),
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
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                if let Some((bg, card, _)) = BRUSHES.with(|b| *b.borrow()) {
                    FillRect(hdc, &ps.rcPaint, bg);
                    let old_brush = SelectObject(hdc, card);
                    let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
                    let radius = 14 * GetDpiForSystem() as i32 / 96;
                    CARDS.with(|c| {
                        for r in c.borrow().iter() {
                            RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius, radius);
                        }
                    });
                    SelectObject(hdc, old_pen);
                    SelectObject(hdc, old_brush);
                }
                EndPaint(hwnd, &ps);
                0
            }
            WM_ERASEBKGND => 1,
            // 컨트롤 글자·배경색: 카드 위 글자는 카드색, 입력칸은 조금 밝게
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                let hdc = wp as HDC;
                let Some((window, card, edit)) = BRUSHES.with(|b| *b.borrow()) else { return 0 };
                let id = GetDlgCtrlID(lp as HWND);
                let (fg, bk, brush) = match msg {
                    WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => (C_TEXT, C_EDIT, edit),
                    _ if id == ID_VERSION => (C_SUBTEXT, C_WINDOW, window),
                    _ if id >= ID_HEADER => (C_SUBTEXT, C_CARD, card),
                    _ => (C_TEXT, C_CARD, card),
                };
                SetTextColor(hdc, fg);
                SetBkColor(hdc, bk);
                brush as LRESULT
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
                BRUSHES.with(|b| {
                    if let Some((a, c, e)) = b.borrow_mut().take() {
                        DeleteObject(a);
                        DeleteObject(c);
                        DeleteObject(e);
                    }
                });
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
