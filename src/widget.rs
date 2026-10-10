//! 바탕화면 위젯: 기기별 원형 게이지 타일 2×2.
//! 반투명 레이어드 창(UpdateLayeredWindow)을 픽셀 단위로 직접 그린다.
//! 바탕화면 창(Progman)을 소유자로 둔 최상위 창이라 항상 바탕화면 바로 위·다른 창들 아래에 있고,
//! 다른 창·게임 위로는 절대 올라오지 않는다. 포커스도 가져가지 않는다.
//! (Progman의 자식으로 넣으면 Windows 11 25H2에서는 반투명 창이 그려지지 않아 쓰지 않는다)

use std::mem::zeroed;
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use razer_battery_tray::gamepad::{PadLevel, PadState, PadStatus};
use razer_battery_tray::parser::{DeviceState, Status};

use crate::tray::{dib, wide};

/// 96 DPI 기준 타일 한 변 / 타일 사이 간격 (px)
const TILE: f32 = 116.0;
const GAP: f32 = 8.0;
const COLS: usize = 2;

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
const G_GAMEPAD: char = '\u{E7FC}';
const G_BOLT: char = '\u{E945}';

const ICON_FONT: &str = "Segoe Fluent Icons";
const NUM_FONT: &str = "Bahnschrift SemiBold";
const LABEL_FONT: &str = "Malgun Gothic";

/// 타일 하나의 표시 내용. 이전과 같으면 다시 그리지 않는다.
#[derive(Clone, PartialEq)]
pub struct Tile {
    glyph: char,
    name: &'static str,
    /// None = 게이지 없음 (꺼짐)
    fraction: Option<f32>,
    color: Rgb,
    text: String,
    /// 숫자(%)면 NUM_FONT, 한글 상태면 LABEL_FONT
    numeric: bool,
    charging: bool,
}

fn level_color(l: u8) -> Rgb {
    if l >= 50 { GREEN } else if l >= 20 { YELLOW } else { RED }
}

pub fn device_tile(d: &DeviceState) -> Tile {
    let (glyph, name) = match d.product_id {
        1365 => (G_HEADSET, "BlackShark V2 Pro"),
        190 => (G_MOUSE, "DeathAdder V4 Pro"),
        122 => (G_MOUSE, "Viper Ultimate"),
        _ => (G_MOUSE, d.name),
    };
    let off = |text: &str| Tile {
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
        (Status::Unknown, _) => off("기록 없음"),
        _ => off("꺼짐"),
    }
}

pub fn pad_tile(p: &PadState) -> Tile {
    let base = Tile {
        glyph: G_GAMEPAD, name: "GameSir G7 Pro", fraction: None, color: SUBTEXT,
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
    drawn: Option<(Vec<Tile>, u32)>,
}

const CLASS: &str = "RazerBatteryWidget";

/// 위치·표시 여부 저장 파일: %LOCALAPPDATA%\RazerBatteryTray\widget.txt ("visible x y")
fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join("RazerBatteryTray").join("widget.txt"))
}

fn load_config() -> (bool, Option<(i32, i32)>) {
    let Some(text) = config_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
        return (true, None);
    };
    let v: Vec<i32> = text.split_whitespace().filter_map(|s| s.parse().ok()).collect();
    match v[..] {
        [vis, x, y] => (vis != 0, Some((x, y))),
        [vis] => (vis != 0, None),
        _ => (true, None),
    }
}

fn save_config(visible: bool, pos: Option<(i32, i32)>) {
    let Some(p) = config_path() else { return };
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let text = match pos {
        Some((x, y)) => format!("{} {x} {y}\n", visible as i32),
        None => format!("{}\n", visible as i32),
    };
    let _ = std::fs::write(p, text);
}

impl Widget {
    pub fn create() -> Self {
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
            let (visible, pos) = load_config();
            // 소유자 = 바탕화면 창. 탐색기가 재시작되면 이 창도 같이 없어지고, TaskbarCreated 때 다시 만든다.
            let progman = FindWindowW(wide("Progman").as_ptr(), null());
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(), class.as_ptr(), WS_POPUP,
                0, 0, 1, 1, progman, null_mut(), hinst, null(),
            );
            Widget { hwnd, visible, initial_pos: Some(pos), drawn: None }
        }
    }

    pub fn empty() -> Self {
        Widget { hwnd: null_mut(), visible: false, initial_pos: None, drawn: None }
    }

    /// 화면 좌표로 창을 옮긴다
    unsafe fn move_to(&self, x: i32, y: i32) {
        unsafe { SetWindowPos(self.hwnd, null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    unsafe fn show(&self) {
        unsafe { SetWindowPos(self.hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW) };
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
        save_config(visible, Some(self.pos()));
    }

    fn pos(&self) -> (i32, i32) {
        let mut r: RECT = unsafe { zeroed() };
        unsafe { GetWindowRect(self.hwnd, &mut r) };
        (r.left, r.top)
    }

    /// 내용이나 DPI가 바뀌었을 때만 다시 그린다.
    pub fn update(&mut self, tiles: Vec<Tile>) {
        if self.hwnd.is_null() {
            return;
        }
        let dpi = unsafe { GetDpiForWindow(self.hwnd) }.max(96);
        if self.drawn.as_ref().is_some_and(|(t, d)| *t == tiles && *d == dpi) {
            return;
        }
        unsafe { self.render(&tiles, dpi) };
        self.drawn = Some((tiles, dpi));
    }

    pub fn invalidate(&mut self) {
        self.drawn = None;
    }

    unsafe fn render(&mut self, tiles: &[Tile], dpi: u32) {
        unsafe {
            let s = dpi as f32 / 96.0;
            let t = (TILE * s).round() as i32;
            let gap = (GAP * s).round() as i32;
            let rows = tiles.len().div_ceil(COLS) as i32;
            let (w, h) = (COLS as i32 * t + (COLS as i32 - 1) * gap, rows * t + (rows - 1) * gap);

            let hdc_screen = GetDC(null_mut());
            let hdc = CreateCompatibleDC(hdc_screen);
            let (bmp, bits) = dib(hdc, w, h);
            let old = SelectObject(hdc, bmp);
            let px = std::slice::from_raw_parts_mut(bits, (w * h) as usize);
            px.fill(0);
            let mut c = Canvas { px, w, h };
            for (i, tile) in tiles.iter().enumerate() {
                let (col, row) = ((i % COLS) as i32, (i / COLS) as i32);
                draw_tile(&mut c, col * (t + gap), row * (t + gap), t, tile);
            }

            // 첫 렌더: 저장된 위치(화면 안에 있을 때만) 또는 작업 영역 오른쪽 아래
            if let Some(saved) = self.initial_pos.take() {
                let on_screen = |&(x, y): &(i32, i32)| {
                    !MonitorFromPoint(POINT { x: x + 20, y: y + 20 }, MONITOR_DEFAULTTONULL).is_null()
                };
                let (x, y) = saved.filter(on_screen).unwrap_or_else(|| {
                    let mut wa: RECT = zeroed();
                    SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut RECT as *mut _, 0);
                    let m = (16.0 * s) as i32;
                    (wa.right - w - m, wa.bottom - h - m)
                });
                self.move_to(x, y);
            }
            let size = SIZE { cx: w, cy: h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8,
            };
            // 위치는 그대로 두고(pptDst = null) 크기와 내용만 갱신
            UpdateLayeredWindow(self.hwnd, hdc_screen, null(), &size, hdc, &src, 0, &blend, ULW_ALPHA);
            // 레이어드 창은 시스템이 비트맵을 복사해 두므로 우리 쪽 버퍼는 바로 해제
            SelectObject(hdc, old);
            DeleteObject(bmp);
            DeleteDC(hdc);
            ReleaseDC(null_mut(), hdc_screen);

            if self.visible {
                self.show();
            }
        }
    }
}

const IDM_HIDE: usize = 10;
const IDM_EXIT: usize = 11;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // 어디를 잡아도 끌어서 옮길 수 있게
            WM_NCHITTEST => HTCAPTION as LRESULT,
            // 항상 맨 아래 (다른 창·게임 위로 올라오지 않음). 소유자(바탕화면)보다는 위에 놓인다.
            WM_WINDOWPOSCHANGING => {
                let pos = &mut *(lp as *mut WINDOWPOS);
                pos.hwndInsertAfter = HWND_BOTTOM;
                pos.flags &= !SWP_NOZORDER;
                0
            }
            WM_EXITSIZEMOVE => {
                let mut r: RECT = zeroed();
                GetWindowRect(hwnd, &mut r);
                save_config(true, Some((r.left, r.top)));
                0
            }
            WM_DPICHANGED => {
                let r = *(lp as *const RECT);
                crate::tray::with_app(|a| {
                    a.widget.move_to(r.left, r.top);
                    a.refresh_widget(true);
                });
                0
            }
            WM_NCRBUTTONUP => {
                let menu = CreatePopupMenu();
                AppendMenuW(menu, MF_STRING, IDM_HIDE, wide("위젯 숨기기").as_ptr());
                AppendMenuW(menu, MF_SEPARATOR, 0, null());
                AppendMenuW(menu, MF_STRING, IDM_EXIT, wide("종료").as_ptr());
                let mut pt: POINT = zeroed();
                GetCursorPos(&mut pt);
                SetForegroundWindow(hwnd);
                let cmd = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY, pt.x, pt.y, 0, hwnd, null());
                PostMessageW(hwnd, WM_NULL, 0, 0);
                DestroyMenu(menu);
                match cmd as usize {
                    IDM_HIDE => {
                        crate::tray::with_app(|a| a.widget.set_visible(false));
                    }
                    IDM_EXIT => crate::tray::quit(),
                    _ => {}
                }
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
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

fn draw_tile(c: &mut Canvas, x0: i32, y0: i32, t: i32, tile: &Tile) {
    let tf = t as f32;
    // 1) 둥근 사각형 배경
    let radius = tf * 0.16;
    for y in 0..t {
        for x in 0..t {
            let a = rounded_rect_alpha(x, y, t, t, radius);
            c.blend(x0 + x, y0 + y, TILE_BG, a * 0.92);
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
    let g = text_run(ICON_FONT, tf * 0.19, &gtext);
    blit(c, &g, x0 + cx as i32, y0 + (cy - tf * 0.07) as i32, glyph_color);

    let (face, em) = if tile.numeric { (NUM_FONT, tf * 0.135) } else { (LABEL_FONT, tf * 0.095) };
    let txt = text_run(face, em, &tile.text);
    let ty = y0 + (cy + tf * 0.12) as i32;
    if tile.charging {
        // 번개 + 글자를 한 줄로 가운데 정렬
        let bolt = text_run(ICON_FONT, em * 0.85, &G_BOLT.to_string());
        let space = (tf * 0.02) as i32;
        let total = bolt.ink_w() + space + txt.ink_w();
        let left = x0 + cx as i32 - total / 2;
        blit(c, &bolt, left + bolt.ink_w() / 2, ty, BLUE);
        blit(c, &txt, left + bolt.ink_w() + space + txt.ink_w() / 2, ty, TEXT);
    } else {
        blit(c, &txt, x0 + cx as i32, ty, if dim { SUBTEXT } else { TEXT });
    }

    // 4) 아래 기기 이름
    let name = text_run(LABEL_FONT, tf * 0.085, tile.name);
    blit(c, &name, x0 + cx as i32, y0 + (tf * 0.88) as i32, SUBTEXT);
}

/// GDI로 그린 글자 커버리지 (회색조 안티에일리어싱) 와 잉크 영역
struct Run {
    w: i32,
    cov: Vec<u8>,
    ink: (i32, i32, i32, i32),
}

impl Run {
    fn ink_w(&self) -> i32 {
        self.ink.2 - self.ink.0
    }
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
