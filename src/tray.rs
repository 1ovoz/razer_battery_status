//! Win32 트레이 아이콘. windows-sys로 직접 호출한다.

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::XboxController::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use razer_battery_tray::gamepad::{PadLevel, PadState, PadStatus};
use razer_battery_tray::parser::{DeviceState, State, Status, N};
use razer_battery_tray::watcher::{default_log_dir, Watcher};

use crate::widget::{device_tile, pad_tile, Widget};

const WM_TRAY: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;
const POLL_MS: u32 = 30_000;
const IDM_EXIT: usize = 1;
const IDM_WIDGET: usize = 2;

/// Razer 기기 N개 + 게임패드 1개
const ICONS: usize = N + 1;
const PAD: usize = N;

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

type Rgb = (u8, u8, u8);
const WHITE: Rgb = (255, 255, 255);
const BLACK: Rgb = (0, 0, 0);
const BLUE: Rgb = (0x1A, 0x73, 0xE8);
const GREEN: Rgb = (0x1E, 0x8E, 0x3E);
const YELLOW: Rgb = (0xF9, 0xC8, 0x0E);
const RED: Rgb = (0xD9, 0x30, 0x25);
const GRAY: Rgb = (0x80, 0x80, 0x80);

#[derive(Clone, PartialEq)]
enum Glyph {
    Text(String),
    /// 4단계 배터리 게이지: 채워진 칸 수 (0..=3)
    Bars(u8),
}

/// 아이콘 모양. 이전과 같으면 다시 그리지 않는다.
#[derive(Clone, PartialEq)]
struct Visual {
    bg: Rgb,
    fg: Rgb,
    glyph: Glyph,
}

pub(crate) struct App {
    hwnd: HWND,
    watcher: Option<Watcher>,
    state: State,
    pad: PadState,
    icon_size: i32,
    icons: [HICON; ICONS],
    drawn: [Option<Visual>; ICONS],
    tips: [String; ICONS],
    pub widget: Widget,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let mut n = 0;
    for c in s.encode_utf16().take(dst.len() - 1) {
        dst[n] = c;
        n += 1;
    }
    dst[n] = 0;
}

pub fn run() {
    unsafe {
        // 중복 실행 방지
        let name = wide("Local\\RazerBatteryTray.SingleInstance");
        let _mutex = CreateMutexW(null(), 0, name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let hinst = GetModuleHandleW(null());
        let class = wide("RazerBatteryTrayWnd");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            ..zeroed()
        };
        RegisterClassW(&wc);
        // TaskbarCreated는 최상위 창에만 브로드캐스트되므로 message-only 창은 쓰지 않는다.
        // 보이지 않는 일반 창(ShowWindow 안 함)을 만든다.
        let hwnd = CreateWindowExW(
            0, class.as_ptr(), class.as_ptr(), WS_OVERLAPPED,
            0, 0, 0, 0, null_mut(), null_mut(), hinst, null(),
        );
        if hwnd.is_null() {
            return;
        }
        TASKBAR_CREATED.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), Ordering::Relaxed);

        let mut app = App {
            hwnd,
            watcher: default_log_dir().map(Watcher::new),
            state: State::default(),
            icon_size: tray_icon_size(),
            pad: PadState::default(),
            icons: [null_mut(); ICONS],
            drawn: Default::default(),
            tips: Default::default(),
            widget: Widget::empty(), // 첫 로그 읽기가 끝난 뒤 refresh_widget에서 만든다
        };
        app.poll();
        app.sync(true);
        app.notify_low();
        app.refresh_widget(false);
        APP.with(|a| *a.borrow_mut() = Some(app));

        SetTimer(hwnd, TIMER_ID, POLL_MS, None);

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
    }
}

pub(crate) fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER if wp == TIMER_ID => {
                with_app(|a| a.refresh());
                0
            }
            WM_TRAY => {
                // NOTIFYICON_VERSION_4: LOWORD(lParam) = 이벤트
                match (lp as u32) & 0xFFFF {
                    WM_CONTEXTMENU => show_menu(hwnd),
                    NIN_SELECT => {
                        with_app(|a| a.refresh());
                    }
                    _ => {}
                }
                0
            }
            WM_DESTROY => {
                KillTimer(hwnd, TIMER_ID);
                with_app(|a| {
                    a.remove_all();
                    DestroyWindow(a.widget.hwnd);
                });
                PostQuitMessage(0);
                0
            }
            m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
                // 탐색기 재시작 → 아이콘 다시 등록 (DPI가 바뀌었을 수 있으니 새로 그림)
                with_app(|a| {
                    let size = tray_icon_size();
                    if size != a.icon_size {
                        a.icon_size = size;
                        a.drawn = Default::default();
                    }
                    a.sync(true);
                    a.refresh_widget(true);
                });
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// 프로그램 종료 (트레이 아이콘·위젯 정리 후 메시지 루프 끝냄)
pub(crate) fn quit() {
    if let Some(hwnd) = with_app(|a| a.hwnd) {
        unsafe { DestroyWindow(hwnd) };
    }
}

unsafe fn show_menu(hwnd: HWND) {
    unsafe {
        let widget_visible = with_app(|a| a.widget.visible).unwrap_or(false);
        let menu = CreatePopupMenu();
        let check = if widget_visible { MF_CHECKED } else { 0 };
        AppendMenuW(menu, MF_STRING | check, IDM_WIDGET, wide("바탕화면 위젯").as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(menu, MF_STRING, IDM_EXIT, wide("종료").as_ptr());
        let mut pt: POINT = zeroed();
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
            pt.x, pt.y, 0, hwnd, null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        match cmd as usize {
            IDM_EXIT => {
                DestroyWindow(hwnd);
            }
            IDM_WIDGET => {
                with_app(|a| {
                    a.widget.set_visible(!widget_visible);
                    a.refresh_widget(false);
                });
            }
            _ => {}
        }
    }
}

fn tray_icon_size() -> i32 {
    unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()).max(16) }
}

impl App {
    fn poll(&mut self) {
        if let Some(w) = &mut self.watcher
            && let Ok(events) = w.poll()
        {
            self.state.apply_all(&events);
        }
        self.pad.update(read_pad(), &now());
    }

    fn refresh(&mut self) {
        self.poll();
        self.sync(false);
        self.notify_low();
        self.refresh_widget(false);
    }

    /// 위젯 내용 갱신 (바뀐 게 없으면 아무것도 안 함). force면 다시 그림.
    pub(crate) fn refresh_widget(&mut self, force: bool) {
        if !self.widget.is_alive() {
            self.widget = Widget::create();
        } else if force {
            self.widget.invalidate();
        }
        let mut tiles: Vec<_> = self.state.devices.iter().map(device_tile).collect();
        tiles.push(pad_tile(&self.pad));
        self.widget.update(tiles);
    }

    fn nid(&self, i: usize) -> NOTIFYICONDATAW {
        let mut nid: NOTIFYICONDATAW = unsafe { zeroed() };
        nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = i as u32;
        nid
    }

    /// 상태가 바뀐 아이콘만 다시 그려 반영한다. add=true면 모두 새로 등록한다.
    fn sync(&mut self, add: bool) {
        for i in 0..ICONS {
            let (visual, tip) = if i == PAD {
                (pad_visual(&self.pad), self.pad.tooltip())
            } else {
                let d = &self.state.devices[i];
                (device_visual(d), d.tooltip())
            };
            let redraw = self.drawn[i].as_ref() != Some(&visual);
            if !add && !redraw && tip == self.tips[i] {
                continue;
            }
            let old = self.icons[i];
            if redraw {
                self.icons[i] = unsafe { render_icon(self.icon_size, &visual) };
                self.drawn[i] = Some(visual);
            }
            let mut nid = self.nid(i);
            nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
            nid.uCallbackMessage = WM_TRAY;
            nid.hIcon = self.icons[i];
            copy_wide(&mut nid.szTip, &tip);
            unsafe {
                if add {
                    Shell_NotifyIconW(NIM_DELETE, &nid); // 이미 있으면 지우고 다시
                    Shell_NotifyIconW(NIM_ADD, &nid);
                    nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                    Shell_NotifyIconW(NIM_SETVERSION, &nid);
                } else {
                    Shell_NotifyIconW(NIM_MODIFY, &nid);
                }
                if redraw && !old.is_null() {
                    DestroyIcon(old);
                }
            }
            self.tips[i] = tip;
        }
    }

    fn notify_low(&mut self) {
        for i in 0..N {
            if self.state.devices[i].take_low_battery_alert() {
                let d = &self.state.devices[i];
                self.balloon(i, &format!("{} 잔량 {}%", d.name, d.level.unwrap_or(0)));
            }
        }
        if self.pad.take_low_battery_alert() {
            self.balloon(PAD, &format!("{} 배터리 거의 없음", razer_battery_tray::gamepad::PAD_NAME));
        }
    }

    fn balloon(&self, i: usize, text: &str) {
        let mut nid = self.nid(i);
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_WARNING;
        copy_wide(&mut nid.szInfoTitle, "배터리 부족");
        copy_wide(&mut nid.szInfo, text);
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
    }

    fn remove_all(&mut self) {
        for i in 0..ICONS {
            let nid = self.nid(i);
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &nid);
                if !self.icons[i].is_null() {
                    DestroyIcon(self.icons[i]);
                }
            }
            self.icons[i] = null_mut();
        }
    }
}

fn text(bg: Rgb, fg: Rgb, t: impl Into<String>) -> Visual {
    Visual { bg, fg, glyph: Glyph::Text(t.into()) }
}

fn device_visual(d: &DeviceState) -> Visual {
    match (d.status, d.level) {
        (Status::Charging, Some(l)) => text(BLUE, WHITE, l.to_string()),
        (Status::Discharging, Some(l)) if l >= 50 => text(GREEN, WHITE, l.to_string()),
        (Status::Discharging, Some(l)) if l >= 20 => text(YELLOW, BLACK, l.to_string()),
        (Status::Discharging, Some(l)) => text(RED, WHITE, l.to_string()),
        _ => text(GRAY, WHITE, "-"),
    }
}

fn pad_visual(p: &PadState) -> Visual {
    let bars = |bg, fg, l: PadLevel| Visual { bg, fg, glyph: Glyph::Bars(l.bars()) };
    match p.status {
        PadStatus::Wireless(l @ (PadLevel::Full | PadLevel::Medium)) => bars(GREEN, WHITE, l),
        PadStatus::Wireless(l @ PadLevel::Low) => bars(YELLOW, BLACK, l),
        PadStatus::Wireless(l) => bars(RED, WHITE, l),
        PadStatus::Wired => text(BLUE, WHITE, "-"),
        PadStatus::Unknown => text(GRAY, WHITE, "?"),
        PadStatus::Disconnected => text(GRAY, WHITE, "-"),
    }
}

/// 연결된 첫 XInput 컨트롤러의 배터리 상태 (2.4G 동글 / 유선 연결된 G7 Pro)
fn read_pad() -> PadStatus {
    for user in 0..4 {
        let mut info: XINPUT_BATTERY_INFORMATION = unsafe { zeroed() };
        let ok = unsafe { XInputGetBatteryInformation(user, BATTERY_DEVTYPE_GAMEPAD, &mut info) } == 0;
        let s = PadStatus::from_xinput(ok, info.BatteryType, info.BatteryLevel);
        if s != PadStatus::Disconnected {
            return s;
        }
    }
    PadStatus::Disconnected
}

/// 현재 시각 "MM/DD HH:MM"
fn now() -> String {
    let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!("{:02}/{:02} {:02}:{:02}", t.wMonth, t.wDay, t.wHour, t.wMinute)
}

pub(crate) unsafe fn dib(hdc: HDC, w: i32, h: i32) -> (HBITMAP, *mut u32) {
    unsafe {
        let mut bmi: BITMAPINFO = zeroed();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h; // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = null_mut();
        let bmp = CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        (bmp, bits as *mut u32)
    }
}

/// 둥근 사각형 배경 위에 숫자(또는 배터리 게이지)를 그린 아이콘을 만든다.
unsafe fn render_icon(size: i32, v: &Visual) -> HICON {
    unsafe {
        // 글자/게이지 커버리지 (0..1)를 만든 뒤 배경색과 합성한다
        let cov = match &v.glyph {
            Glyph::Text(t) => text_coverage(size, t),
            Glyph::Bars(n) => bars_coverage(size, *n),
        };
        let hdc = CreateCompatibleDC(null_mut());
        let (color_bmp, px) = dib(hdc, size, size);
        DeleteDC(hdc);
        let px = std::slice::from_raw_parts_mut(px, (size * size) as usize);
        let r = size as f32 / 5.0;
        for y in 0..size {
            for x in 0..size {
                let a = corner_alpha(x, y, size, r);
                let c = cov[(y * size + x) as usize];
                let mix = |b: u8, f: u8| (b as f32 + (f as f32 - b as f32) * c).round() as u32;
                let (rr, gg, bb) = (mix(v.bg.0, v.fg.0), mix(v.bg.1, v.fg.1), mix(v.bg.2, v.fg.2));
                // straight alpha BGRA
                px[(y * size + x) as usize] = ((a * 255.0).round() as u32) << 24 | rr << 16 | gg << 8 | bb;
            }
        }

        let mask = CreateBitmap(size, size, 1, 1, null());
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color_bmp };
        let icon = CreateIconIndirect(&info);
        DeleteObject(mask);
        DeleteObject(color_bmp);
        icon
    }
}

/// GDI 글자는 알파를 쓰지 않으므로, 검은 버퍼에 흰 글자를 그려 그 밝기를 커버리지로 쓴다.
/// 넉넉한 임시 버퍼에 그리고 아이콘 안에 들어갈 때까지 글꼴을 줄인 뒤, 잉크 영역 기준으로 가운데 정렬한다.
unsafe fn text_coverage(size: i32, text: &str) -> Vec<f32> {
    unsafe {
        let wtext: Vec<u16> = text.encode_utf16().collect();
        let (tw, th) = (size * 3, size * 2);
        let hdc = CreateCompatibleDC(null_mut());
        let (tmp_bmp, tmp) = dib(hdc, tw, th);
        let old_bmp = SelectObject(hdc, tmp_bmp);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, 0x00FF_FFFF);
        let face = wide("Bahnschrift SemiBold SemiConden");
        let tmp_px = std::slice::from_raw_parts_mut(tmp, (tw * th) as usize);
        let limit = if text.len() >= 3 { size } else { size - 2 };
        let mut bbox = (0, 0, 0, 0);
        let mut em = size + 2;
        while em >= 6 {
            tmp_px.fill(0);
            let font = CreateFontW(
                -em, 0, 0, 0, FW_NORMAL as i32, 0, 0, 0, DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32, CLIP_DEFAULT_PRECIS as u32,
                ANTIALIASED_QUALITY as u32, DEFAULT_PITCH as u32, face.as_ptr(),
            );
            let old_font = SelectObject(hdc, font);
            TextOutW(hdc, size / 2, 0, wtext.as_ptr(), wtext.len() as i32);
            GdiFlush();
            SelectObject(hdc, old_font);
            DeleteObject(font);
            bbox = ink_bbox(tmp_px, tw, th);
            let (x0, y0, x1, y1) = bbox;
            if x1 - x0 <= limit && y1 - y0 <= size - 2 {
                break;
            }
            em -= 1;
        }

        let (x0, y0, x1, y1) = bbox;
        let ox = x0 - (size - (x1 - x0)) / 2;
        let oy = y0 - (size - (y1 - y0) + 1) / 2;
        let mut cov = vec![0f32; (size * size) as usize];
        for y in 0..size {
            for x in 0..size {
                let (sx, sy) = (x + ox, y + oy);
                if sx >= 0 && sy >= 0 && sx < tw && sy < th {
                    cov[(y * size + x) as usize] = ((tmp_px[(sy * tw + sx) as usize] >> 8) & 0xFF) as f32 / 255.0;
                }
            }
        }
        SelectObject(hdc, old_bmp);
        DeleteObject(tmp_bmp);
        DeleteDC(hdc);
        cov
    }
}

/// 세로 막대 3칸짜리 배터리 게이지. 채워진 칸은 진하게, 빈 칸은 흐리게.
fn bars_coverage(size: i32, filled: u8) -> Vec<f32> {
    let gap = (size / 16).max(1);
    let bw = (size - 2 * (size / 8) - 2 * gap) / 3;
    let total = 3 * bw + 2 * gap;
    let x_start = (size - total) / 2;
    let (y0, y1) = (size * 3 / 16, size - size * 3 / 16);
    let mut cov = vec![0f32; (size * size) as usize];
    for b in 0..3 {
        let c = if (b as u8) < filled { 1.0 } else { 0.3 };
        let bx = x_start + b * (bw + gap);
        for y in y0..y1 {
            for x in bx..bx + bw {
                cov[(y * size + x) as usize] = c;
            }
        }
    }
    cov
}

/// 글자 잉크가 있는 영역 (x0, y0, x1, y1), x1/y1은 미포함
fn ink_bbox(px: &[u32], w: i32, h: i32) -> (i32, i32, i32, i32) {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if (px[(y * w + x) as usize] >> 8) & 0xFF > 48 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 { (0, 0, 0, 0) } else { (x0, y0, x1, y1) }
}

/// 둥근 모서리 안티에일리어싱 알파 (0..1)
fn corner_alpha(x: i32, y: i32, size: i32, r: f32) -> f32 {
    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
    let s = size as f32;
    let cx = if fx < r { r } else if fx > s - r { s - r } else { return 1.0 };
    let cy = if fy < r { r } else if fy > s - r { s - r } else { return 1.0 };
    let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
    (r - d + 0.5).clamp(0.0, 1.0)
}
