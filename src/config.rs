//! 사용자 설정과 잔량 부족 알림 주기. 파일 형식은 한 줄에 key=value.
//! Win32 의존성이 없어 어느 플랫폼에서나 테스트할 수 있다.

use std::path::{Path, PathBuf};

/// 장치 수: Razer 3개 + GameSir 1개 (TRACKED 순서 뒤에 패드)
pub const DEVICES: usize = crate::parser::N + 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remind {
    Off,
    Once,
    /// 분 단위 반복
    Every(u32),
}

impl Remind {
    /// 설정 화면 선택지 순서
    pub const CHOICES: [Remind; 7] = [
        Remind::Every(1), Remind::Every(3), Remind::Every(5), Remind::Every(15), Remind::Every(30),
        Remind::Once, Remind::Off,
    ];

    pub fn label(self) -> String {
        match self {
            Remind::Off => "알림 안 함".into(),
            Remind::Once => "1번만".into(),
            Remind::Every(m) => format!("{m}분마다"),
        }
    }

    fn encode(self) -> String {
        match self {
            Remind::Off => "off".into(),
            Remind::Once => "once".into(),
            Remind::Every(m) => m.to_string(),
        }
    }

    fn decode(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Remind::Off),
            "once" => Some(Remind::Once),
            n => n.parse().ok().filter(|&m| m > 0).map(Remind::Every),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Grid,
    Horizontal,
    Vertical,
}

impl Layout {
    /// 보이는 타일 n개를 놓을 열 수
    pub fn cols(self, n: usize) -> usize {
        match self {
            Layout::Grid => n.clamp(1, 2),
            Layout::Horizontal => n.max(1),
            Layout::Vertical => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// 위젯 타일 순서 (장치 번호의 순열)
    pub order: [usize; DEVICES],
    pub show_widget: [bool; DEVICES],
    pub show_tray: [bool; DEVICES],
    /// 비어 있으면 기본 이름
    pub nickname: [String; DEVICES],
    /// 이 % 미만이면 잔량 부족
    pub low_threshold: u8,
    pub remind: Remind,
    /// 위젯 배경 불투명도 0..=100 (글자·게이지는 영향 없음)
    pub bg_opacity: u8,
    pub layout: Layout,
    /// 위젯 전체 크기 % (60..=200)
    pub scale: u16,
    pub widget_visible: bool,
    /// 로그·컨트롤러 확인 주기 (초)
    pub poll_secs: u32,
    /// 하루 한 번 새 버전이 있는지 확인해 알림 (설치는 직접)
    pub update_check: bool,
    /// 위젯 화면 좌표 (없으면 기본 위치)
    pub pos: Option<(i32, i32)>,
}

/// 갱신 주기 선택지 (초). 5초가 기본·추천.
pub const POLL_CHOICES: [u32; 5] = [1, 5, 15, 30, 60];
pub const POLL_DEFAULT: u32 = 5;

pub const SCALE_MIN: u16 = 60;
pub const SCALE_MAX: u16 = 200;

impl Default for Config {
    fn default() -> Self {
        Self {
            order: std::array::from_fn(|i| i),
            show_widget: [true; DEVICES],
            show_tray: [true; DEVICES],
            nickname: Default::default(),
            low_threshold: 15,
            remind: Remind::Once,
            bg_opacity: 92,
            layout: Layout::Grid,
            scale: 100,
            widget_visible: true,
            poll_secs: POLL_DEFAULT,
            update_check: true,
            pos: None,
        }
    }
}

fn bools(v: &[bool]) -> String {
    v.iter().map(|&b| if b { "1" } else { "0" }).collect::<Vec<_>>().join(",")
}

fn parse_bools(s: &str, out: &mut [bool]) {
    for (o, p) in out.iter_mut().zip(s.split(',')) {
        *o = p.trim() != "0";
    }
}

impl Config {
    pub fn parse(text: &str) -> Self {
        let mut c = Config::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim_end_matches('\r'));
            match k {
                "order" => {
                    let v: Vec<usize> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
                    // 0..DEVICES를 한 번씩 담은 순열일 때만 받아들인다
                    let mut seen = [false; DEVICES];
                    if v.len() == DEVICES && v.iter().all(|&i| i < DEVICES && !std::mem::replace(&mut seen[i], true)) {
                        c.order.copy_from_slice(&v);
                    }
                }
                "show_widget" => parse_bools(v, &mut c.show_widget),
                "show_tray" => parse_bools(v, &mut c.show_tray),
                "low_threshold" => c.low_threshold = v.parse().unwrap_or(15).clamp(1, 99),
                "remind" => c.remind = Remind::decode(v).unwrap_or(Remind::Once),
                "bg_opacity" => c.bg_opacity = v.parse::<u32>().map_or(92, |o| o.min(100) as u8),
                "layout" => {
                    c.layout = match v {
                        "horizontal" => Layout::Horizontal,
                        "vertical" => Layout::Vertical,
                        _ => Layout::Grid,
                    }
                }
                "scale" => c.scale = v.parse::<u32>().map_or(100, |s| s.clamp(SCALE_MIN as u32, SCALE_MAX as u32) as u16),
                "widget_visible" => c.widget_visible = v != "0",
                // auto_update는 1.1.x(자동 업데이트 시절)의 이름
                "update_check" | "auto_update" => c.update_check = v != "0",
                "poll_secs" => c.poll_secs = v.parse::<u32>().map_or(POLL_DEFAULT, |p| p.clamp(1, 3600)),
                "pos" => {
                    c.pos = v.split_once(',').and_then(|(x, y)| Some((x.trim().parse().ok()?, y.trim().parse().ok()?)))
                }
                _ => {
                    if let Some(i) = k.strip_prefix("nickname").and_then(|n| n.parse::<usize>().ok())
                        && i < DEVICES
                    {
                        // 설정 창 입력 제한과 같은 40자까지만 (손으로 고친 파일 대비)
                        c.nickname[i] = v.trim().chars().take(40).collect();
                    }
                }
            }
        }
        c
    }

    pub fn serialize(&self) -> String {
        let mut s = format!(
            "order={}\nshow_widget={}\nshow_tray={}\nlow_threshold={}\nremind={}\nbg_opacity={}\nlayout={}\nscale={}\nwidget_visible={}\npoll_secs={}\nupdate_check={}\n",
            self.order.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(","),
            bools(&self.show_widget),
            bools(&self.show_tray),
            self.low_threshold,
            self.remind.encode(),
            self.bg_opacity,
            match self.layout {
                Layout::Grid => "grid",
                Layout::Horizontal => "horizontal",
                Layout::Vertical => "vertical",
            },
            self.scale,
            self.widget_visible as u8,
            self.poll_secs,
            self.update_check as u8,
        );
        if let Some((x, y)) = self.pos {
            s += &format!("pos={x},{y}\n");
        }
        for (i, n) in self.nickname.iter().enumerate() {
            // 줄바꿈은 저장할 수 없으니 공백으로
            s += &format!("nickname{i}={}\n", n.replace(['\r', '\n'], " "));
        }
        s
    }

    /// %LOCALAPPDATA%\RazerBatteryTray\settings.ini
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("LOCALAPPDATA")?;
        Some(Path::new(&base).join("RazerBatteryTray").join("settings.ini"))
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.serialize());
    }

    /// 표시 이름: 닉네임이 있으면 닉네임, 없으면 기본 이름
    pub fn display_name<'a>(&'a self, i: usize, default: &'a str) -> &'a str {
        let n = self.nickname[i].trim();
        if n.is_empty() { default } else { n }
    }
}

/// 장치 하나의 잔량 부족 알림 상태
#[derive(Default, Clone, Debug)]
pub struct Reminder {
    last: Option<u64>,
}

impl Reminder {
    /// low: Some(true)=부족, Some(false)=정상(충전 중 포함), None=꺼짐·모름(상태 유지).
    /// now: 초 단위 단조 시각. 알림을 띄워야 하면 true.
    pub fn check(&mut self, low: Option<bool>, now: u64, remind: Remind) -> bool {
        match low {
            None => false,
            Some(false) => {
                self.last = None;
                false
            }
            Some(true) => {
                let fire = match (remind, self.last) {
                    (Remind::Off, _) => false,
                    (_, None) => true,
                    (Remind::Once, Some(_)) => false,
                    (Remind::Every(m), Some(t)) => now.saturating_sub(t) >= m as u64 * 60,
                };
                if fire {
                    self.last = Some(now);
                }
                fire
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut c = Config::default();
        c.order.reverse();
        c.show_widget[1] = false;
        for (i, v) in c.show_tray.iter_mut().enumerate() {
            *v = i % 2 == 0;
        }
        c.nickname[2] = "내 마우스 = 메인".into();
        c.low_threshold = 20;
        c.remind = Remind::Every(5);
        c.bg_opacity = 40;
        c.layout = Layout::Vertical;
        c.scale = 150;
        c.widget_visible = false;
        c.poll_secs = 15;
        c.update_check = false;
        c.pos = Some((-1200, 300));
        assert_eq!(Config::parse(&c.serialize()), c);
        assert_eq!(c.display_name(2, "Viper"), "내 마우스 = 메인");
        assert_eq!(c.display_name(0, "DeathAdder"), "DeathAdder");
    }

    #[test]
    fn bad_values_fall_back() {
        let c = Config::parse("scale=9999\nbg_opacity=300\nremind=0\nlayout=?\nshow_tray=0\npoll_secs=0\ngarbage\n");
        assert_eq!(c.poll_secs, 1);
        assert_eq!(Config::parse("poll_secs=abc").poll_secs, POLL_DEFAULT);
        assert_eq!(c.scale, SCALE_MAX);
        assert_eq!(c.bg_opacity, 100);
        assert_eq!(c.remind, Remind::Once);
        assert_eq!(c.layout, Layout::Grid);
        assert!(!c.show_tray[0] && c.show_tray[1..].iter().all(|&b| b));
    }

    #[test]
    fn order_must_be_permutation() {
        // 장치 수(DEVICES)가 바뀌어도 그대로 돌아가게 문자열을 만들어 쓴다
        let list = |v: &[usize]| format!("order={}", v.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(","));
        let default = Config::default().order;
        let mut reversed = default;
        reversed.reverse();
        assert_eq!(Config::parse(&list(&reversed)).order, reversed);
        let mut dup = default;
        dup[1] = dup[0];
        assert_eq!(Config::parse(&list(&dup)).order, default, "중복");
        assert_eq!(Config::parse(&list(&default[1..])).order, default, "개수 부족");
        let mut out = default;
        out[0] = DEVICES + 5;
        assert_eq!(Config::parse(&list(&out)).order, default, "범위 밖");
    }

    #[test]
    fn reads_old_auto_update_key() {
        assert!(!Config::parse("auto_update=0").update_check);
        assert!(Config::parse("").update_check);
    }

    #[test]
    fn layout_columns() {
        assert_eq!(Layout::Grid.cols(4), 2);
        assert_eq!(Layout::Grid.cols(1), 1);
        assert_eq!(Layout::Horizontal.cols(3), 3);
        assert_eq!(Layout::Vertical.cols(4), 1);
    }

    #[test]
    fn remind_once() {
        let mut r = Reminder::default();
        assert!(!r.check(Some(false), 0, Remind::Once));
        assert!(r.check(Some(true), 10, Remind::Once));
        assert!(!r.check(Some(true), 10_000, Remind::Once));
        assert!(!r.check(None, 10_100, Remind::Once), "꺼져도 다시 알리지 않음");
        assert!(!r.check(Some(true), 10_200, Remind::Once));
        assert!(!r.check(Some(false), 10_300, Remind::Once)); // 충전/회복 → 다시 무장
        assert!(r.check(Some(true), 10_400, Remind::Once));
    }

    #[test]
    fn remind_every() {
        let mut r = Reminder::default();
        assert!(r.check(Some(true), 0, Remind::Every(3)));
        assert!(!r.check(Some(true), 179, Remind::Every(3)));
        assert!(r.check(Some(true), 180, Remind::Every(3)));
        assert!(!r.check(Some(true), 200, Remind::Every(3)));
    }

    #[test]
    fn remind_off() {
        let mut r = Reminder::default();
        assert!(!r.check(Some(true), 0, Remind::Off));
        assert!(!r.check(Some(true), 99_999, Remind::Off));
    }
}
