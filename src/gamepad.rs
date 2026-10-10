//! XInput 컨트롤러(GameSir G7 Pro 2.4G 동글 / 유선) 배터리 상태.
//! XInput은 잔량을 %가 아니라 4단계(EMPTY/LOW/MEDIUM/FULL)로만 알려준다.
//! Win32 호출은 tray.rs에서 하고, 여기서는 해석과 표시 문자열만 다룬다.

pub const PAD_NAME: &str = "GameSir G7 Pro";
pub const PAD_SHORT_NAME: &str = "GameSir G7 Pro";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PadLevel {
    Empty,
    Low,
    Medium,
    Full,
}

impl PadLevel {
    /// XInput BATTERY_LEVEL_* 값 (0..=3)
    pub fn from_xinput(level: u8) -> Self {
        match level {
            0 => Self::Empty,
            1 => Self::Low,
            2 => Self::Medium,
            _ => Self::Full,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Empty => "거의 없음",
            Self::Low => "낮음",
            Self::Medium => "보통",
            Self::Full => "높음",
        }
    }

    /// 아이콘에 채울 칸 수 (0..=3)
    pub fn bars(self) -> u8 {
        self as u8
    }

    /// 잔량 부족 판정용 대략적인 %
    pub fn approx_percent(self) -> u8 {
        match self {
            Self::Empty => 5,
            Self::Low => 25,
            Self::Medium => 60,
            Self::Full => 100,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadStatus {
    /// XInput 슬롯에 컨트롤러 없음 (꺼짐 / 동글 미연결)
    Disconnected,
    /// 케이블 연결 (충전 중)
    Wired,
    Wireless(PadLevel),
    /// 연결됐지만 배터리 종류를 알려주지 않음
    Unknown,
}

impl PadStatus {
    /// XInputGetBatteryInformation 결과 해석. ok=false면 연결 안 됨.
    pub fn from_xinput(ok: bool, battery_type: u8, battery_level: u8) -> Self {
        match (ok, battery_type) {
            (false, _) | (true, 0) => Self::Disconnected, // BATTERY_TYPE_DISCONNECTED
            (true, 1) => Self::Wired,                      // BATTERY_TYPE_WIRED
            (true, 2 | 3) => Self::Wireless(PadLevel::from_xinput(battery_level)), // ALKALINE, NIMH
            _ => Self::Unknown,                            // BATTERY_TYPE_UNKNOWN 등
        }
    }
}

#[derive(Clone, Debug)]
pub struct PadState {
    pub status: PadStatus,
    /// 상태가 마지막으로 바뀐 시각 ("10/10 18:20")
    pub changed_at: Option<String>,
    /// 꺼지기 전 마지막 무선 잔량과 그 시각
    pub last_wireless: Option<(PadLevel, String)>,
}

impl Default for PadState {
    fn default() -> Self {
        Self { status: PadStatus::Disconnected, changed_at: None, last_wireless: None }
    }
}

impl PadState {
    /// 새로 읽은 상태를 반영한다. now = 현재 시각 ("MM/DD HH:MM"). 바뀌었으면 true.
    pub fn update(&mut self, status: PadStatus, now: &str) -> bool {
        if let PadStatus::Wireless(l) = status {
            self.last_wireless = Some((l, now.to_string()));
        }
        if status == self.status {
            return false;
        }
        self.status = status;
        self.changed_at = Some(now.to_string());
        true
    }

    /// 잔량 부족 여부 (parser::DeviceState::low와 같은 의미). 단계를 대략적인 %로 바꿔 비교한다.
    pub fn low(&self, threshold: u8) -> Option<bool> {
        match self.status {
            PadStatus::Wireless(l) => Some(l.approx_percent() < threshold),
            PadStatus::Wired => Some(false),
            _ => None,
        }
    }

    pub fn tooltip(&self, name: &str) -> String {
        let at = self.changed_at.as_deref().unwrap_or("");
        match self.status {
            PadStatus::Disconnected => match &self.last_wireless {
                Some((l, t)) => format!("{name}\n꺼짐 (마지막 {}, {t})", l.label()),
                None => format!("{name}\n연결 안 됨"),
            },
            PadStatus::Wired => format!("{name}\n유선 연결 (충전 중)\n갱신 {at}"),
            PadStatus::Wireless(l) => {
                format!("{name}\n배터리 {} ({}/3칸)\n갱신 {at}", l.label(), l.bars())
            }
            PadStatus::Unknown => format!("{name}\n연결됨 (잔량 정보 없음)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_xinput_values() {
        assert_eq!(PadStatus::from_xinput(false, 2, 3), PadStatus::Disconnected);
        assert_eq!(PadStatus::from_xinput(true, 0, 0), PadStatus::Disconnected);
        assert_eq!(PadStatus::from_xinput(true, 1, 3), PadStatus::Wired);
        assert_eq!(PadStatus::from_xinput(true, 3, 1), PadStatus::Wireless(PadLevel::Low));
        assert_eq!(PadStatus::from_xinput(true, 2, 2), PadStatus::Wireless(PadLevel::Medium));
        assert_eq!(PadStatus::from_xinput(true, 255, 0), PadStatus::Unknown);
    }

    #[test]
    fn off_keeps_last_wireless_level() {
        let mut p = PadState::default();
        assert_eq!(p.tooltip(PAD_NAME), "GameSir G7 Pro\n연결 안 됨");
        assert!(p.update(PadStatus::Wireless(PadLevel::Medium), "10/10 18:20"));
        assert!(!p.update(PadStatus::Wireless(PadLevel::Medium), "10/10 18:21"));
        assert!(p.update(PadStatus::Disconnected, "10/10 18:30"));
        assert_eq!(p.tooltip(PAD_NAME), "GameSir G7 Pro\n꺼짐 (마지막 보통, 10/10 18:21)");
    }

    #[test]
    fn low_from_levels() {
        let mut p = PadState::default();
        assert_eq!(p.low(15), None);
        p.update(PadStatus::Wireless(PadLevel::Low), "t");
        assert_eq!(p.low(15), Some(false));
        assert_eq!(p.low(30), Some(true));
        p.update(PadStatus::Wireless(PadLevel::Empty), "t");
        assert_eq!(p.low(15), Some(true));
        p.update(PadStatus::Wired, "t");
        assert_eq!(p.low(15), Some(false));
    }
}
