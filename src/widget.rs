//! 바탕화면 위젯: 기기별 원형 게이지 타일 (2×2 / 가로 / 세로).
//! 반투명 레이어드 창(UpdateLayeredWindow)을 픽셀 단위로 직접 그린다.
//! 바탕화면 창(Progman)을 소유자로 둔 최상위 창이라 항상 바탕화면 바로 위·다른 창들 아래에 있고,
//! 다른 창·게임 위로는 절대 올라오지 않는다. 포커스도 가져가지 않는다.
//! (Progman의 자식으로 넣으면 Windows 11 25H2에서는 반투명 창이 그려지지 않아 쓰지 않는다)
//!
//! 마우스를 올리면 타일 바깥 옆쪽에 톱니바퀴(설정)와 세로 배경 불투명도 바가 나타난다
//! (평소에는 그 자리가 완전히 투명). 화면 오른쪽에 있으면 왼쪽에, 왼쪽에 있으면 오른쪽에 붙는다.
//! 휠로 전체 크기, 오른쪽 클릭 메뉴로 배치·크기를 바꾼다.

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use razer_battery_tray::config::{Layout, SCALE_MAX, SCALE_MIN};
use razer_battery_tray::gamepad::{PadLevel, PadState, PadStatus};
use razer_battery_tray::parser::{DeviceState, Kind, Status};

use crate::tray::{dib, wide, with_app};

/// 96 DPI·크기 100% 기준 타일 한 변 / 타일 사이 간격 (px)
const TILE: f32 = 116.0;
const GAP: f32 = 8.0;

type Rgb = (u8, u8, u8);
const TILE_BG: Rgb = (22, 22, 24);
const TRACK: Rgb = (58, 58, 62);
const TEXT: Rgb = (240, 240, 240);
const SUBTEXT: Rgb = (150, 150, 155);
const GREEN: Rgb = (0x34, 0xC7, 0x59);
const YELLOW: Rgb = (0xF9, 0xC8, 0x0E);
const RED: Rgb = (0xFF, 0x45, 0x3A);
const BLUE: Rgb = (0x0A, 0x84, 0xFF);

// Segoe Fluent Icons 글리프
const G_MOUSE: char = '\u{E962}';
const G_HEADSET: char = '\u{E7F6}';
const G_KEYBOARD: char = '\u{E765}';
const G_DEVICE: char = '\u{E772}';
const G_GAMEPAD: char = '\u{E7FC}';
const G_BOLT: char = '\u{E945}';
const G_GEAR: char = '\u{E713}';

/// 아이콘 글꼴: Windows 11은 Segoe Fluent Icons, 없으면(Windows 10) 같은 글리프 코드를 쓰는 Segoe MDL2 Assets
fn icon_font() -> &'static str {
    static FONT: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    FONT.get_or_init(|| unsafe {
        const FLUENT: &str = "Segoe Fluent Icons";
        let hdc = CreateCompatibleDC(null_mut());
        let font = CreateFontW(
            -16, 0, 0, 0, FW_NORMAL as i32, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32, ANTIALIASED_QUALITY as u32, DEFAULT_PITCH as u32, wide(FLUENT).as_ptr(),
        );
        let old = SelectObject(hdc, font);
        let mut buf = [0u16; 64];
        let n = GetTextFaceW(hdc, buf.len() as i32, buf.as_mut_ptr());
        SelectObject(hdc, old);
        DeleteObject(font);
        DeleteDC(hdc);
        // 글꼴이 없으면 GDI가 다른 글꼴로 바꿔치기하므로 실제 이름으로 확인
        let actual = String::from_utf16_lossy(&buf[..n.max(1) as usize - 1]);
        if actual.eq_ignore_ascii_case(FLUENT) { FLUENT } else { "Segoe MDL2 Assets" }
    })
}
const NUM_FONT: &str = "Bahnschrift SemiBold";
const LABEL_FONT: &str = "Malgun Gothic";


/// 타일 하나의 표시 내용. 이전과 같으면 다시 그리지 않는다.
#[derive(Clone, PartialEq)]
pub struct Tile {
    glyph: char,
    name: String,
    /// None = 게이지 없음 (꺼짐)
    fraction: Option<f32>,
    color: Rgb,
    text: String,
    /// 숫자(%)면 NUM_FONT, 한글 상태면 LABEL_FONT
    numeric: bool,
    charging: bool,
}

/// 위젯 전체 모양 설정
#[derive(Clone, Copy, PartialEq)]
pub struct Style {
    pub bg_opacity: u8,
    pub layout: Layout,
    pub scale: u16,
}

/// 50% 이상 초록, 15~49% 노랑, 15% 미만 빨강
fn level_color(l: u8) -> Rgb {
    if l >= 50 { GREEN } else if l >= 15 { YELLOW } else { RED }
}

pub fn device_tile(d: &DeviceState, name: &str) -> Tile {
    let glyph = match d.kind {
        Kind::Mouse => G_MOUSE,
        Kind::Headset => G_HEADSET,
        Kind::Keyboard => G_KEYBOARD,
        Kind::Other => G_DEVICE,
    };
    let name = name.to_string();
    let off = |name: String, text: &str| Tile {
        glyph, name, fraction: None, color: SUBTEXT, text: text.into(), numeric: false, charging: false,
    };
    match (d.status, d.level) {
        (Status::Charging, Some(l)) => Tile {
            glyph, name, fraction: Some(l as f32 / 100.0), color: BLUE,
            text: format!("{l}%"), numeric: true, charging: true,
        },
        (Status::Discharging, Some(l)) => Tile {
            glyph, name, fraction: Some(l as f32 / 100.0), color: level_color(l),
            text: format!("{l}%"), numeric: true, charging: false,
        },
        (Status::Unknown, _) => off(name, "기록 없음"),
        _ => off(name, "꺼짐"),
    }
}

pub fn pad_tile(p: &PadState, name: &str) -> Tile {
    let base = Tile {
        glyph: G_GAMEPAD, name: name.to_string(), fraction: None, color: SUBTEXT,
        text: String::new(), numeric: false, charging: false,
    };
    match p.status {
        PadStatus::Wireless(l) => {
            let (fraction, color) = match l {
                PadLevel::Full => (1.0, GREEN),
                PadLevel::Medium => (0.66, GREEN),
                PadLevel::Low => (0.33, YELLOW),
                PadLevel::Empty => (0.05, RED),
            };
            Tile { fraction: Some(fraction), color, text: l.label().into(), ..base }
        }
        PadStatus::Wired => Tile { fraction: Some(1.0), color: BLUE, text: "유선".into(), charging: true, ..base },
        PadStatus::Unknown => Tile { text: "연결됨".into(), ..base },
        PadStatus::Disconnected => Tile { text: "꺼짐".into(), ..base },
    }
}

pub struct Widget {
    pub hwnd: HWND,
    pub visible: bool,
    /// 아직 자리를 잡지 않았으면 저장된 화면 좌표 (없으면 기본 위치)
    initial_pos: Option<Option<(i32, i32)>>,
    /// 마지막으로 그린 내용 (타일, 모양, DPI, 호버 여부)
    drawn: Option<(Vec<Tile>, Style, u32, bool)>,
    /// 마지막으로 받은 내용 (호버가 바뀌면 이걸로 다시 그림)
    last: Option<(Vec<Tile>, Style)>,
    /// 타일 영역 크기와, 창 안에서 타일 영역이 시작하는 x (패널이 왼쪽이면 0보다 큼)
    tiles_size: (i32, i32),
    tile_off: i32,
    pub hovered: bool,
    pub dragging_slider: bool,
    /// 창 기준 좌표: 톱니바퀴 / 불투명도 바 (호버 중에만 유효)
    gear: RECT,
    slider: RECT,
}

const CLASS: &str = "RazerBatteryWidget";

impl Widget {
    pub fn create(visible: bool, pos: Option<(i32, i32)>) -> Self {
        unsafe {
            let hinst = GetModuleHandleW(null());
            let class = wide(CLASS);
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                lpszClassName: class.as_ptr(),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                ..zeroed()
            };
            RegisterClassW(&wc); // 두 번째 호출(탐색기 재시작 후 재생성)은 실패해도 무방
            // 소유자 = 바탕화면 창. 탐색기가 재시작되면 이 창도 같이 없어지고, TaskbarCreated 때 다시 만든다.
            let progman = FindWindowW(wide("Progman").as_ptr(), null());
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(), class.as_ptr(), WS_POPUP,
                0, 0, 1, 1, progman, null_mut(), hinst, null(),
            );
            Widget { hwnd, visible, initial_pos: Some(pos), ..Self::empty() }
        }
    }

    pub fn empty() -> Self {
        let r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        Widget {
            hwnd: null_mut(), visible: false, initial_pos: None, drawn: None, last: None,
            tiles_size: (0, 0), tile_off: 0, hovered: false, dragging_slider: false, gear: r, slider: r,
        }
    }

    pub fn is_alive(&self) -> bool {
        !self.hwnd.is_null() && unsafe { IsWindow(self.hwnd) } != 0
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        unsafe {
            if visible {
                self.show();
            } else {
                ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    unsafe fn show(&self) {
        unsafe { SetWindowPos(self.hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW) };
    }

    pub fn pos(&self) -> (i32, i32) {
        let mut r: RECT = unsafe { zeroed() };
        unsafe { GetWindowRect(self.hwnd, &mut r) };
        (r.left, r.top)
    }

    /// 타일 영역의 화면 좌표 (설정에 저장하는 위치)
    pub fn tiles_pos(&self) -> (i32, i32) {
        let (x, y) = self.pos();
        (x + self.tile_off, y)
    }

    /// 내용·모양·DPI·호버가 바뀌었을 때만 다시 그린다.
    /// 크기가 바뀌어 창을 옮겼으면 새 화면 좌표를 돌려준다 (설정에 저장용).
    pub fn update(&mut self, tiles: Vec<Tile>, style: Style) -> Option<(i32, i32)> {
        if self.hwnd.is_null() {
            return None;
        }
        let dpi = unsafe { GetDpiForWindow(self.hwnd) }.max(96);
        let hovered = self.hovered || self.dragging_slider;
        if self.drawn.as_ref().is_some_and(|(t, s, d, h)| *t == tiles && *s == style && *d == dpi && *h == hovered) {
            return None;
        }
        self.drawn = Some((tiles.clone(), style, dpi, hovered));
        self.last = Some((tiles.clone(), style));
        unsafe { self.render(&tiles, style, dpi, hovered) }
    }

    /// 호버 상태만 바뀌었을 때 마지막 내용으로 다시 그림
    pub fn redraw(&mut self) {
        if let Some((t, s)) = self.last.clone() {
            self.update(t, s);
        }
    }

    pub fn invalidate(&mut self) {
        self.drawn = None;
    }

    /// 화면 좌표 → 창 기준 좌표
    fn to_local(&self, x: i32, y: i32) -> (i32, i32) {
        let (wx, wy) = self.pos();
        (x - wx, y - wy)
    }

    fn in_rect(r: &RECT, (x, y): (i32, i32)) -> bool {
        x >= r.left && x < r.right && y >= r.top && y < r.bottom
    }

    /// 화면 좌표의 y로 불투명도 값 (바 위쪽 = 100)
    fn slider_value(&self, screen_y: i32) -> u8 {
        let (_, y) = self.to_local(0, screen_y);
        let (top, bottom) = (self.slider.top + 6, self.slider.bottom - 6);
        let f = 1.0 - (y - top) as f32 / (bottom - top).max(1) as f32;
        (f.clamp(0.0, 1.0) * 100.0).round() as u8
    }

    unsafe fn render(&mut self, tiles: &[Tile], style: Style, dpi: u32, hovered: bool) -> Option<(i32, i32)> {
        unsafe {
            let s = dpi as f32 / 96.0 * style.scale as f32 / 100.0;
            let t = (TILE * s).round() as i32;
            let tf = t as f32;
            let gap = (GAP * s).round().max(2.0) as i32;
            let n = tiles.len().max(1);
            let cols = style.layout.cols(n);
            let rows = n.div_ceil(cols) as i32;
            let cols = cols as i32;
            let (tw, th) = (cols * t + (cols - 1) * gap, rows * t + (rows - 1) * gap);

            // 1) 타일 영역의 화면 위치 정하기
            let mut moved = None;
            let (tx, ty) = if let Some(saved) = self.initial_pos.take() {
                // 첫 렌더: 저장된 위치(화면 안에 있을 때만) 또는 작업 영역 오른쪽 아래
                let on_screen = |&(x, y): &(i32, i32)| {
                    !MonitorFromPoint(POINT { x: x + 20, y: y + 20 }, MONITOR_DEFAULTTONULL).is_null()
                };
                saved.filter(on_screen).unwrap_or_else(|| {
                    let mut wa: RECT = zeroed();
                    SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut RECT as *mut _, 0);
                    let m = (16.0 * dpi as f32 / 96.0) as i32;
                    (wa.right - tw - m, wa.bottom - th - m)
                })
            } else {
                let (x, y) = self.tiles_pos();
                let (ow, oh) = self.tiles_size;
                if (ow, oh) != (0, 0) && (ow, oh) != (tw, th) {
                    // 크기가 바뀌면 화면 가장자리 쪽 모서리를 고정 (오른쪽/아래에 있으면 그쪽 기준)
                    let wa = self.work_area();
                    let nx = if x + ow / 2 > (wa.left + wa.right) / 2 { x + ow - tw } else { x };
                    let ny = if y + oh / 2 > (wa.top + wa.bottom) / 2 { y + oh - th } else { y };
                    if (nx, ny) != (x, y) {
                        moved = Some((nx, ny));
                    }
                    (nx, ny)
                } else {
                    (x, y)
                }
            };

            // 2) 톱니바퀴·불투명도 바 패널: 화면 가운데 쪽 옆에 붙인다
            let pw = (tf * 0.22).round() as i32;
            let wa = self.work_area();
            let panel_left = tx + tw / 2 > (wa.left + wa.right) / 2;
            let (w, h) = (tw + gap + pw, th);
            let tile_off = if panel_left { pw + gap } else { 0 };
            let px0 = if panel_left { 0 } else { tw + gap };

            let hdc_screen = GetDC(null_mut());
            let hdc = CreateCompatibleDC(hdc_screen);
            let (bmp, bits) = dib(hdc, w, h);
            if bits.is_null() {
                // 메모리 부족 등: 이번에는 그리지 않고 다음 갱신 때 다시 시도
                DeleteObject(bmp);
                DeleteDC(hdc);
                ReleaseDC(null_mut(), hdc_screen);
                self.drawn = None;
                return None;
            }
            let old = SelectObject(hdc, bmp);
            let px = std::slice::from_raw_parts_mut(bits, (w * h) as usize);
            // 호버 중에는 창 전체를 알파 1/255로 채워(눈에 안 보임) 타일 사이 틈에서도 마우스를 놓치지 않게 한다
            px.fill(if hovered { 0x0100_0000 } else { 0 });
            let mut c = Canvas { px, w, h };
            // 배경 알파가 0이면 클릭이 통과해 잡을 수 없으므로 최소 1/255는 남긴다 (눈에는 안 보임)
            let bg_alpha = (style.bg_opacity as f32 / 100.0).max(1.0 / 255.0);
            for (i, tile) in tiles.iter().enumerate() {
                let (col, row) = ((i as i32 % cols), (i as i32 / cols));
                draw_tile(&mut c, tile_off + col * (t + gap), row * (t + gap), t, tile, bg_alpha);
            }

            let ph = th.min(2 * t + gap);
            self.gear = RECT { left: px0, top: 0, right: px0 + pw, bottom: pw };
            self.slider = RECT { left: px0, top: pw, right: px0 + pw, bottom: ph - pw / 4 };
            if hovered {
                let r = pw as f32 * 0.5;
                for y in 0..ph {
                    for x in 0..pw {
                        c.blend(px0 + x, y, TILE_BG, rounded_rect_alpha(x, y, pw, ph, r) * 0.9);
                    }
                }
                let g = text_run(icon_font(), pw as f32 * 0.55, &G_GEAR.to_string());
                blit(&mut c, &g, px0 + pw / 2, pw / 2, TEXT);
                draw_slider(&mut c, &self.slider, style.bg_opacity, tf);
            }

            // 3) 창 위치: 타일 영역이 (tx, ty)에 오도록
            let (wx, wy) = (tx - tile_off, ty);
            if (wx, wy) != self.pos() {
                SetWindowPos(self.hwnd, null_mut(), wx, wy, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
            self.tiles_size = (tw, th);
            self.tile_off = tile_off;

            let size = SIZE { cx: w, cy: h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8,
            };
            // 위치는 위에서 맞췄으니(pptDst = null) 크기와 내용만 갱신
            UpdateLayeredWindow(self.hwnd, hdc_screen, null(), &size, hdc, &src, 0, &blend, ULW_ALPHA);
            // 레이어드 창은 시스템이 비트맵을 복사해 두므로 우리 쪽 버퍼는 바로 해제
            SelectObject(hdc, old);
            DeleteObject(bmp);
            DeleteDC(hdc);
            ReleaseDC(null_mut(), hdc_screen);

            if self.visible {
                self.show();
            }
            moved
        }
    }

    /// 위젯이 있는 모니터의 작업 영역
    unsafe fn work_area(&self) -> RECT {
        unsafe {
            let mut mi: MONITORINFO = zeroed();
            mi.cbSize = size_of::<MONITORINFO>() as u32;
            GetMonitorInfoW(MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST), &mut mi);
            mi.rcWork
        }
    }
}

const WM_MOUSELEAVE: u32 = 0x02A3;

const IDM_HIDE: usize = 10;
const IDM_EXIT: usize = 11;
const IDM_SETTINGS: usize = 12;
const IDM_LAYOUT: usize = 20; // +0 2×2, +1 가로, +2 세로
const IDM_SIZE: usize = 30; // +i → SIZES[i]
const SIZES: [(u16, &str); 4] = [(75, "작게"), (100, "보통"), (125, "크게"), (150, "아주 크게")];
const LAYOUTS: [(Layout, &str); 3] = [(Layout::Grid, "2×2"), (Layout::Horizontal, "가로 한 줄"), (Layout::Vertical, "세로 한 줄")];

/// 마우스 추적 (영역을 벗어나면 WM_MOUSELEAVE / WM_NCMOUSELEAVE)
unsafe fn track_leave(hwnd: HWND, nonclient: bool) {
    unsafe {
        let mut tme = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE | if nonclient { TME_NONCLIENT } else { 0 },
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        TrackMouseEvent(&mut tme);
    }
}

fn cursor() -> (i32, i32) {
    let mut pt = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut pt) };
    (pt.x, pt.y)
}

unsafe fn show_widget_menu(hwnd: HWND) {
    unsafe {
        let (layout, scale) = with_app(|a| (a.config.layout, a.config.scale)).unwrap_or((Layout::Grid, 100));
        let menu = CreatePopupMenu();
        let sub_layout = CreatePopupMenu();
        for (i, (l, label)) in LAYOUTS.iter().enumerate() {
            let check = if *l == layout { MF_CHECKED } else { 0 };
            AppendMenuW(sub_layout, MF_STRING | check, IDM_LAYOUT + i, wide(label).as_ptr());
        }
        let sub_size = CreatePopupMenu();
        for (i, (sz, label)) in SIZES.iter().enumerate() {
            let check = if *sz == scale { MF_CHECKED } else { 0 };
            AppendMenuW(sub_size, MF_STRING | check, IDM_SIZE + i, wide(&format!("{label} ({sz}%)")).as_ptr());
        }
        AppendMenuW(menu, MF_POPUP, sub_layout as usize, wide("배치").as_ptr());
        AppendMenuW(menu, MF_POPUP, sub_size as usize, wide("크기 (휠로도 조절)").as_ptr());
        AppendMenuW(menu, MF_STRING, IDM_SETTINGS, wide("설정...").as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(menu, MF_STRING, IDM_HIDE, wide("위젯 숨기기").as_ptr());
        AppendMenuW(menu, MF_STRING, IDM_EXIT, wide("종료").as_ptr());
        let (x, y) = cursor();
        SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY, x, y, 0, hwnd, null()) as usize;
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu); // 하위 메뉴도 같이 해제됨
        match cmd {
            IDM_HIDE => {
                with_app(|a| a.set_widget_visible(false));
            }
            IDM_EXIT => crate::tray::quit(),
            IDM_SETTINGS => crate::tray::open_settings(),
            c if (IDM_LAYOUT..IDM_LAYOUT + LAYOUTS.len()).contains(&c) => {
                with_app(|a| a.change_config(|cfg| cfg.layout = LAYOUTS[c - IDM_LAYOUT].0));
            }
            c if (IDM_SIZE..IDM_SIZE + SIZES.len()).contains(&c) => {
                with_app(|a| a.change_config(|cfg| cfg.scale = SIZES[c - IDM_SIZE].0));
            }
            _ => {}
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let screen_pt = || ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
        match msg {
            // 호버 중 톱니바퀴·바 위는 클릭용(HTCLIENT), 나머지는 어디를 잡아도 끌어서 옮김
            WM_NCHITTEST => {
                let p = screen_pt();
                let on_control = with_app(|a| {
                    let w = &a.widget;
                    let l = w.to_local(p.0, p.1);
                    (w.hovered || w.dragging_slider) && (Widget::in_rect(&w.gear, l) || Widget::in_rect(&w.slider, l))
                })
                .unwrap_or(false);
                (if on_control { HTCLIENT } else { HTCAPTION }) as LRESULT
            }
            WM_MOUSEMOVE | WM_NCMOUSEMOVE => {
                track_leave(hwnd, msg == WM_NCMOUSEMOVE);
                with_app(|a| {
                    if a.widget.dragging_slider {
                        let v = a.widget.slider_value(cursor().1);
                        if v != a.config.bg_opacity {
                            a.config.bg_opacity = v;
                            a.refresh_widget(false);
                        }
                    } else if !a.widget.hovered {
                        a.widget.hovered = true;
                        a.widget.redraw();
                    }
                });
                if msg == WM_MOUSEMOVE { 0 } else { DefWindowProcW(hwnd, msg, wp, lp) }
            }
            WM_MOUSELEAVE | WM_NCMOUSELEAVE => {
                // 클라이언트↔비클라이언트 영역 사이를 옮길 때도 오므로 실제로 창 밖인지 확인
                let (x, y) = cursor();
                let mut r: RECT = zeroed();
                GetWindowRect(hwnd, &mut r);
                let outside = x < r.left || x >= r.right || y < r.top || y >= r.bottom;
                if outside {
                    with_app(|a| {
                        if a.widget.hovered && !a.widget.dragging_slider {
                            a.widget.hovered = false;
                            a.widget.redraw();
                        }
                    });
                }
                0
            }
            WM_LBUTTONDOWN => {
                let (x, y) = cursor();
                let on_gear = with_app(|a| {
                    let l = a.widget.to_local(x, y);
                    if Widget::in_rect(&a.widget.slider, l) {
                        a.widget.dragging_slider = true;
                        a.config.bg_opacity = a.widget.slider_value(y);
                        a.refresh_widget(false);
                        false
                    } else {
                        Widget::in_rect(&a.widget.gear, l)
                    }
                })
                .unwrap_or(false);
                if on_gear {
                    crate::tray::open_settings();
                } else {
                    SetCapture(hwnd);
                }
                0
            }
            WM_LBUTTONUP => {
                ReleaseCapture();
                with_app(|a| {
                    if a.widget.dragging_slider {
                        a.widget.dragging_slider = false;
                        a.save_config();
                        a.widget.redraw();
                    }
                });
                0
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp >> 16) & 0xFFFF) as i16 as i32;
                let step = if delta > 0 { 10 } else { -10 };
                with_app(|a| {
                    a.change_config(|c| c.scale = (c.scale as i32 + step).clamp(SCALE_MIN as i32, SCALE_MAX as i32) as u16)
                });
                0
            }
            // 항상 맨 아래 (다른 창·게임 위로 올라오지 않음). 소유자(바탕화면)보다는 위에 놓인다.
            WM_WINDOWPOSCHANGING => {
                let pos = &mut *(lp as *mut WINDOWPOS);
                pos.hwndInsertAfter = HWND_BOTTOM;
                pos.flags &= !SWP_NOZORDER;
                0
            }
            WM_EXITSIZEMOVE => {
                with_app(|a| {
                    let p = a.widget.tiles_pos();
                    // 화면 반대쪽으로 옮겼으면 패널 방향이 바뀌므로 다시 그림
                    a.widget.invalidate();
                    a.change_config(|c| c.pos = Some(p));
                });
                0
            }
            WM_DPICHANGED => {
                let r = *(lp as *const RECT);
                SetWindowPos(hwnd, null_mut(), r.left, r.top, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                with_app(|a| a.refresh_widget(true));
                0
            }
            WM_NCRBUTTONUP | WM_RBUTTONUP => {
                show_widget_menu(hwnd);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// 세로 불투명도 바: 트랙 + 채워진 부분 + 손잡이
fn draw_slider(c: &mut Canvas, r: &RECT, value: u8, tf: f32) {
    let cx = (r.left + r.right) as f32 / 2.0;
    let (top, bottom) = ((r.top + 6) as f32, (r.bottom - 6) as f32);
    let half = (tf * 0.012).max(1.5);
    let knob_y = bottom - (bottom - top) * value as f32 / 100.0;
    let knob_r = (tf * 0.035).max(4.0);
    for y in r.top..r.bottom {
        for x in r.left..r.right {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // 둥근 끝 막대
            let dy = if fy < top { top - fy } else if fy > bottom { fy - bottom } else { 0.0 };
            let d = ((fx - cx).powi(2) + dy * dy).sqrt();
            let bar = (half - d + 0.5).clamp(0.0, 1.0);
            if bar > 0.0 {
                let col = if fy >= knob_y { TEXT } else { TRACK };
                c.blend(x, y, col, bar);
            }
            let k = (knob_r - ((fx - cx).powi(2) + (fy - knob_y).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0);
            c.blend(x, y, TEXT, k);
        }
    }
}

// ───────────── 그리기 ─────────────

/// premultiplied BGRA 픽셀 버퍼
struct Canvas<'a> {
    px: &'a mut [u32],
    w: i32,
    h: i32,
}

impl Canvas<'_> {
    /// (x, y)에 색을 알파 a(0..1)로 덮어 그린다 (premultiplied over)
    fn blend(&mut self, x: i32, y: i32, c: Rgb, a: f32) {
        if a <= 0.0 || x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let a = a.min(1.0);
        let i = (y * self.w + x) as usize;
        let d = self.px[i];
        let inv = 1.0 - a;
        let ch = |shift: u32, src: f32| -> u32 {
            let dv = ((d >> shift) & 0xFF) as f32;
            ((src * a + dv * inv).round() as u32).min(255) << shift
        };
        self.px[i] = ch(24, 255.0) | ch(16, c.0 as f32) | ch(8, c.1 as f32) | ch(0, c.2 as f32);
    }
}

fn draw_tile(c: &mut Canvas, x0: i32, y0: i32, t: i32, tile: &Tile, bg_alpha: f32) {
    let tf = t as f32;
    // 1) 둥근 사각형 배경
    let radius = tf * 0.16;
    for y in 0..t {
        for x in 0..t {
            let a = rounded_rect_alpha(x, y, t, t, radius);
            c.blend(x0 + x, y0 + y, TILE_BG, a * bg_alpha);
        }
    }

    // 2) 원형 게이지 (12시 방향에서 시계 방향)
    let (cx, cy) = (tf / 2.0, tf * 0.43);
    let r = tf * 0.30;
    let half = tf * 0.036;
    let end = tile.fraction.map(|f| f.clamp(0.0, 1.0) * std::f32::consts::TAU);
    let cap = |ang: f32, fx: f32, fy: f32| {
        let (px, py) = (cx + r * ang.sin(), cy - r * ang.cos());
        (half - ((fx - px).powi(2) + (fy - py).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0)
    };
    let (gx0, gx1) = ((cx - r - half - 2.0) as i32, (cx + r + half + 2.0) as i32);
    let (gy0, gy1) = ((cy - r - half - 2.0) as i32, (cy + r + half + 2.0) as i32);
    for y in gy0..=gy1 {
        for x in gx0..=gx1 {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let (dx, dy) = (fx - cx, fy - cy);
            let ring = (half - ((dx * dx + dy * dy).sqrt() - r).abs() + 0.5).clamp(0.0, 1.0);
            c.blend(x0 + x, y0 + y, TRACK, ring);
            if let Some(end) = end {
                let mut ang = dx.atan2(-dy);
                if ang < 0.0 {
                    ang += std::f32::consts::TAU;
                }
                let mut cov = if ang <= end { ring } else { 0.0 };
                cov = cov.max(cap(0.0, fx, fy)).max(cap(end, fx, fy));
                c.blend(x0 + x, y0 + y, tile.color, cov);
            }
        }
    }

    // 3) 가운데 기기 아이콘 + 아래 잔량 글자
    let dim = tile.fraction.is_none();
    let glyph_color = if dim { SUBTEXT } else { TEXT };
    let gtext = tile.glyph.to_string();
    let g = text_run(icon_font(), tf * 0.19, &gtext);
    blit(c, &g, x0 + cx as i32, y0 + (cy - tf * 0.07) as i32, glyph_color);

    let (face, em) = if tile.numeric { (NUM_FONT, tf * 0.135) } else { (LABEL_FONT, tf * 0.095) };
    let txt = text_run(face, em, &tile.text);
    let ty = y0 + (cy + tf * 0.12) as i32;
    blit(c, &txt, x0 + cx as i32, ty, if dim { SUBTEXT } else { TEXT });

    // 충전 중이면 타일 오른쪽 위 모서리에 번개
    if tile.charging {
        let bolt = text_run(icon_font(), tf * 0.14, &G_BOLT.to_string());
        blit(c, &bolt, x0 + t - (tf * 0.13) as i32, y0 + (tf * 0.13) as i32, BLUE);
    }

    // 4) 아래 기기 이름
    let name = text_run(LABEL_FONT, tf * 0.085, &tile.name);
    blit(c, &name, x0 + cx as i32, y0 + (tf * 0.88) as i32, SUBTEXT);
}

/// GDI로 그린 글자 커버리지 (회색조 안티에일리어싱) 와 잉크 영역
struct Run {
    w: i32,
    cov: Vec<u8>,
    ink: (i32, i32, i32, i32),
}

fn text_run(face: &str, em: f32, text: &str) -> Run {
    unsafe {
        let wtext: Vec<u16> = text.encode_utf16().collect();
        let hdc = CreateCompatibleDC(null_mut());
        let wface = wide(face);
        let font = CreateFontW(
            -(em.round() as i32), 0, 0, 0, FW_NORMAL as i32, 0, 0, 0, DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32, CLIP_DEFAULT_PRECIS as u32, ANTIALIASED_QUALITY as u32,
            DEFAULT_PITCH as u32, wface.as_ptr(),
        );
        let old_font = SelectObject(hdc, font);
        let mut sz: SIZE = zeroed();
        GetTextExtentPoint32W(hdc, wtext.as_ptr(), wtext.len() as i32, &mut sz);
        // 기울어진 글리프가 잘리지 않게 여유를 둔다
        let (w, h) = (sz.cx + 4, sz.cy + 2);
        let (bmp, bits) = dib(hdc, w.max(1), h.max(1));
        if bits.is_null() {
            DeleteObject(bmp);
            SelectObject(hdc, old_font);
            DeleteObject(font);
            DeleteDC(hdc);
            return Run { w: 1, cov: vec![0], ink: (0, 0, 0, 0) };
        }
        let old_bmp = SelectObject(hdc, bmp);
        let px = std::slice::from_raw_parts_mut(bits, (w * h).max(1) as usize);
        px.fill(0);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, 0x00FF_FFFF);
        TextOutW(hdc, 2, 1, wtext.as_ptr(), wtext.len() as i32);
        GdiFlush();
        let cov: Vec<u8> = px.iter().map(|p| ((p >> 8) & 0xFF) as u8).collect();
        SelectObject(hdc, old_bmp);
        DeleteObject(bmp);
        SelectObject(hdc, old_font);
        DeleteObject(font);
        DeleteDC(hdc);

        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for y in 0..h {
            for x in 0..w {
                if cov[(y * w + x) as usize] > 24 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        let ink = if x1 > x0 { (x0, y0, x1, y1) } else { (0, 0, 0, 0) };
        Run { w, cov, ink }
    }
}

/// 잉크 영역의 가운데가 (cx, cy)에 오도록 그린다
fn blit(c: &mut Canvas, run: &Run, cx: i32, cy: i32, color: Rgb) {
    let (x0, y0, x1, y1) = run.ink;
    let ox = cx - (x0 + x1) / 2;
    let oy = cy - (y0 + y1) / 2;
    for y in y0..y1 {
        for x in x0..x1 {
            let a = run.cov[(y * run.w + x) as usize];
            if a > 0 {
                c.blend(ox + x, oy + y, color, a as f32 / 255.0);
            }
        }
    }
}

/// 둥근 사각형 안티에일리어싱 알파 (0..1)
fn rounded_rect_alpha(x: i32, y: i32, w: i32, h: i32, r: f32) -> f32 {
    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
    let (wf, hf) = (w as f32, h as f32);
    let cx = if fx < r { r } else if fx > wf - r { wf - r } else { return 1.0 };
    let cy = if fy < r { r } else if fy > hf - r { hf - r } else { return 1.0 };
    let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
    (r - d + 0.5).clamp(0.0, 1.0)
}
