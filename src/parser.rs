//! Razer Synapse 4 systray 로그 한 줄을 해석하고, 기기 상태를 갱신한다.
//! Win32 의존성이 없어 어느 플랫폼에서나 테스트할 수 있다.

use serde::Deserialize;

/// 위젯 아이콘 종류
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mouse,
    Headset,
    Keyboard,
    Other,
}

/// 추적할 Razer 기기 하나
#[derive(Clone, Copy, Debug)]
pub struct Tracked {
    /// Synapse 로그의 "productId" 값
    pub product_id: u32,
    /// 트레이 툴팁·알림에 쓰는 이름
    pub name: &'static str,
    /// 위젯 타일 아래에 쓰는 짧은 이름
    pub short_name: &'static str,
    pub kind: Kind,
}

/// 추적 대상 기기. 기기를 추가하려면 여기에 한 줄만 넣으면 된다 (README 참고).
pub const TRACKED: [Tracked; 3] = [
    Tracked { product_id: 190, name: "Razer DeathAdder V4 Pro", short_name: "DeathAdder V4 Pro", kind: Kind::Mouse },
    Tracked { product_id: 1365, name: "Razer BlackShark V2 Pro", short_name: "BlackShark V2 Pro", kind: Kind::Headset },
    Tracked { product_id: 122, name: "Razer Viper Ultimate", short_name: "Viper Ultimate", kind: Kind::Mouse },
];
pub const N: usize = TRACKED.len();

/// "2026/10/10 17:38:33.237" — 사전순 비교 = 시간순 비교
pub type Ts = [u8; 23];

/// 스냅샷 안에서 한 기기에 대해 관찰한 내용
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Obs {
    /// 목록에는 있지만 powerStatus가 없음 (연결 직후 등) → 상태 유지
    NoPower,
    /// chargingStatus == "off" → level은 믿지 않음
    Off,
    On { level: u8, charging: bool },
}

/// 기기 목록 스냅샷 하나. obs[i]가 None이면 TRACKED[i]가 목록에 없음(= 연결 끊김).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub ts: Ts,
    pub obs: [Option<Obs>; N],
}

#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    /// 기기 목록 태그가 아닌 줄
    Irrelevant,
    /// 기기 목록 태그지만 빈 배열이거나 productId가 없는 목록
    NotDeviceList,
    Snapshot(Event),
    /// 기기 목록 태그인데 JSON 해석 실패
    Malformed,
}

#[derive(Deserialize)]
struct PowerStatus<'a> {
    #[serde(rename = "chargingStatus", borrow)]
    charging_status: Option<std::borrow::Cow<'a, str>>,
    level: Option<f64>,
}

#[derive(Deserialize)]
struct Device<'a> {
    #[serde(rename = "productId")]
    product_id: Option<u32>,
    #[serde(rename = "hasBattery")]
    has_battery: Option<bool>,
    #[serde(rename = "powerStatus", borrow)]
    power_status: Option<PowerStatus<'a>>,
}

#[derive(Deserialize)]
struct Wrapper<'a> {
    #[serde(borrow)]
    devices: Vec<Device<'a>>,
}

const TAGS: [&[u8]; 3] = [b"getConnectingDevices", b"mapDevices", b"SYNAPSE_DEVICES_SET"];

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// 로그 한 줄(끝의 \r\n 포함 가능)을 해석한다.
pub fn parse_line(line: &[u8]) -> Line {
    // [YYYY/MM/DD HH:MM:SS.mmm] level: ...
    if line.len() < 26 || line[0] != b'[' || line[24] != b']' {
        return Line::Irrelevant;
    }
    let mut ts = [0u8; 23];
    ts.copy_from_slice(&line[1..24]);

    // 메시지 본문: "] info: " 다음
    let rest = &line[25..];
    let Some(colon) = find(rest, b": ") else { return Line::Irrelevant };
    let msg = &rest[colon + 2..];

    // JSON 시작 = 처음 나오는 "[{" 또는 "{"
    let json_start = match (find(msg, b"[{"), msg.iter().position(|&b| b == b'{')) {
        (Some(a), Some(b)) => a.min(b),
        (a, b) => match a.or(b) {
            Some(i) => i,
            None => return Line::Irrelevant, // "Device  []" 같은 빈 목록 포함
        },
    };
    let tag = &msg[..json_start];
    let is_list_tag = TAGS.iter().any(|t| find(tag, t).is_some())
        || tag.trim_ascii() == b"Device";
    if !is_list_tag {
        return Line::Irrelevant;
    }

    let json = &msg[json_start..];
    let mut de = serde_json::Deserializer::from_slice(json);
    let devices: Vec<Device> = if json[0] == b'[' {
        match Vec::<Device>::deserialize(&mut de) {
            Ok(v) => v,
            Err(_) => return Line::Malformed,
        }
    } else {
        match Wrapper::deserialize(&mut de) {
            Ok(w) => w.devices,
            Err(_) => return Line::Malformed,
        }
    };

    if !devices.iter().any(|d| d.product_id.is_some()) {
        return Line::NotDeviceList;
    }

    let mut obs = [None; N];
    for d in &devices {
        let Some(pid) = d.product_id else { continue };
        let Some(i) = TRACKED.iter().position(|t| t.product_id == pid) else { continue };
        let o = match (&d.power_status, d.has_battery) {
            (_, Some(false)) | (None, _) => Obs::NoPower,
            (Some(ps), _) => match ps.charging_status.as_deref() {
                Some(s) if s.eq_ignore_ascii_case("off") => Obs::Off,
                Some(s) => match ps.level {
                    Some(l) => Obs::On {
                        level: l.round().clamp(0.0, 100.0) as u8,
                        charging: s.eq_ignore_ascii_case("charging"),
                    },
                    None => Obs::NoPower,
                },
                None => Obs::NoPower,
            },
        };
        // 같은 기기가 한 목록에 두 번 나오면 정보가 많은 쪽을 쓴다
        if obs[i].is_none() || obs[i] == Some(Obs::NoPower) {
            obs[i] = Some(o);
        }
    }
    Line::Snapshot(Event { ts, obs })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// 아직 로그에서 본 적 없음
    Unknown,
    Discharging,
    Charging,
    Off,
}

#[derive(Clone, Debug)]
pub struct DeviceState {
    pub product_id: u32,
    pub name: &'static str,
    pub kind: Kind,
    pub status: Status,
    /// 마지막으로 관찰한 실제 잔량 (off일 때의 0은 포함하지 않음)
    pub level: Option<u8>,
    /// level을 관찰한 시각
    pub level_ts: Option<Ts>,
}

impl DeviceState {
    /// 잔량 부족 여부. Some(true)=threshold% 미만으로 사용 중, Some(false)=정상·충전 중, None=꺼짐·모름
    pub fn low(&self, threshold: u8) -> Option<bool> {
        match (self.status, self.level) {
            (Status::Discharging, Some(l)) => Some(l < threshold),
            (Status::Charging, _) => Some(false),
            _ => None,
        }
    }

    /// 트레이 툴팁 문자열 (name = 표시 이름)
    pub fn tooltip(&self, name: &str) -> String {
        let when = |ts: &Option<Ts>| ts.map(|t| fmt_ts(&t)).unwrap_or_default();
        match (self.status, self.level) {
            (Status::Unknown, _) => format!("{name}\n기록 없음"),
            (Status::Off, Some(l)) => format!("{name}\n꺼짐 (마지막 {l}%, {})", when(&self.level_ts)),
            (Status::Off, None) => format!("{name}\n꺼짐"),
            (Status::Charging, Some(l)) => format!("{name}\n{l}% 충전 중\n갱신 {}", when(&self.level_ts)),
            (_, Some(l)) => format!("{name}\n{l}%\n갱신 {}", when(&self.level_ts)),
            (_, None) => format!("{name}\n잔량 정보 없음"),
        }
    }
}

/// "2026/10/10 17:38:33.237" → "10/10 17:38"
pub fn fmt_ts(ts: &Ts) -> String {
    let s = std::str::from_utf8(ts).unwrap_or("");
    match (s.get(5..10), s.get(11..16)) {
        (Some(d), Some(t)) => format!("{d} {t}"),
        _ => s.to_string(),
    }
}

pub struct State {
    pub devices: [DeviceState; N],
    /// 마지막으로 적용한 스냅샷 시각. 이보다 오래된 스냅샷은 무시한다.
    pub last_ts: Option<Ts>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            devices: TRACKED.map(|t| DeviceState {
                product_id: t.product_id,
                name: t.name,
                kind: t.kind,
                status: Status::Unknown,
                level: None,
                level_ts: None,
            }),
            last_ts: None,
        }
    }
}

impl State {
    /// 시각 순으로 정렬된 스냅샷을 적용한다.
    pub fn apply(&mut self, ev: &Event) {
        if self.last_ts.is_some_and(|t| ev.ts < t) {
            return;
        }
        self.last_ts = Some(ev.ts);
        for (d, o) in self.devices.iter_mut().zip(ev.obs.iter()) {
            match o {
                // 목록에 없음 = 연결 끊김
                None => {
                    if d.status != Status::Unknown || d.level.is_some() {
                        d.status = Status::Off;
                    }
                }
                Some(Obs::NoPower) => {}
                Some(Obs::Off) => d.status = Status::Off,
                Some(Obs::On { level, charging }) => {
                    d.status = if *charging { Status::Charging } else { Status::Discharging };
                    d.level = Some(*level);
                    d.level_ts = Some(ev.ts);
                }
            }
        }
    }

    pub fn apply_all(&mut self, events: &[Event]) {
        for ev in events {
            self.apply(ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(pid: u32, status: &str, level: u32) -> String {
        format!(
            r#"{{"serialNumber":"X","productId":{pid},"profiles":[{{"name":"a"}}],"hasBattery":true,"name":{{"en":"n"}},"powerStatus":{{"chargingStatus":"{status}","level":{level}}}}}"#
        )
    }

    fn snap(ts: &str, devs: &[String]) -> Event {
        let line = format!(
            "[{ts}] info: [getConnectingDevices] ~ file: Synapse.js:55 ~ connectingDeviceData: [{}]\r\n",
            devs.join(",")
        );
        match parse_line(line.as_bytes()) {
            Line::Snapshot(e) => e,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_all_tag_forms() {
        let d = dev(122, "NoCharge_BatteryFull", 67);
        let lines = [
            format!("[2026/10/10 17:38:33.237] info: [getConnectingDevices] ~ file: Synapse.js:55 ~ connectingDeviceData: [{d}]"),
            format!("[2026/10/10 17:38:33.237] info: 🚀 ~ mapDevices ~ devices: [{d}]"),
            format!(r#"[2026/10/10 17:38:33.237] info: [SYNAPSE_DEVICES_SET] ~ file: synapse-actions.js:12: {{"devices":[{d}]}}"#),
            format!("[2026/10/10 17:38:33.237] info: Device  [{d}]\r\n"),
        ];
        for l in &lines {
            let Line::Snapshot(e) = parse_line(l.as_bytes()) else { panic!("{l}") };
            assert_eq!(&e.ts, b"2026/10/10 17:38:33.237");
            assert_eq!(e.obs[2], Some(Obs::On { level: 67, charging: false }));
            assert_eq!(e.obs[0], None);
        }
    }

    #[test]
    fn ignores_non_lists() {
        for l in [
            "[2026/10/10 17:38:33.237] info: Device  []",
            "[2026/10/10 17:38:33.237] info: 🚀 ~ mapDevices ~ devices: []",
            "[2026/10/10 17:38:33.237] info: [RazerApp] RazerApp.on(\"broadcastMessage\", {\"productId\":122})",
            "[2026/10/10 17:38:33.237] info: [[page-running-garbage]]",
            "garbage",
            "",
        ] {
            assert_eq!(parse_line(l.as_bytes()), Line::Irrelevant, "{l}");
        }
        for l in [
            r#"[2026/10/10 17:38:33.237] info: [SYNAPSE_DEVICES_SET] ~ file: x.js:12: {"devices":[]}"#,
            r#"[2026/10/10 17:38:33.237] info: Device  [{"foo":1}]"#,
        ] {
            assert_eq!(parse_line(l.as_bytes()), Line::NotDeviceList, "{l}");
        }
        assert_eq!(
            parse_line(br#"[2026/10/10 17:38:33.237] info: Device  [{"productId":122,"#),
            Line::Malformed
        );
    }

    #[test]
    fn off_keeps_last_real_level() {
        let mut s = State::default();
        s.apply(&snap("2026/10/09 23:51:00.000", &[dev(122, "NoCharge_BatteryFull", 79)]));
        s.apply(&snap("2026/10/09 23:59:00.000", &[dev(122, "off", 0)]));
        let d = &s.devices[2];
        assert_eq!(d.status, Status::Off);
        assert_eq!(d.level, Some(79));
        assert_eq!(d.tooltip(d.name), "Razer Viper Ultimate\n꺼짐 (마지막 79%, 10/09 23:51)");
        assert_eq!(d.tooltip("내 마우스"), "내 마우스\n꺼짐 (마지막 79%, 10/09 23:51)");
    }

    #[test]
    fn absent_from_list_means_off() {
        let mut s = State::default();
        s.apply(&snap("2026/10/09 10:00:00.000", &[dev(190, "Charging", 40), dev(122, "NoCharge_BatteryFull", 50)]));
        assert_eq!(s.devices[0].status, Status::Charging);
        s.apply(&snap("2026/10/09 11:00:00.000", &[dev(122, "NoCharge_BatteryFull", 49)]));
        assert_eq!(s.devices[0].status, Status::Off);
        assert_eq!(s.devices[0].level, Some(40));
        // 한 번도 본 적 없는 기기는 Unknown 유지
        assert_eq!(s.devices[1].status, Status::Unknown);
        // 빈 목록은 무시되므로 상태가 바뀌지 않음
        assert_eq!(parse_line(b"[2026/10/09 12:00:00.000] info: Device  []"), Line::Irrelevant);
        assert_eq!(s.devices[2].status, Status::Discharging);
    }

    #[test]
    fn nocharge_full_is_not_full() {
        let mut s = State::default();
        s.apply(&snap("2026/10/09 10:00:00.000", &[dev(1365, "NoCharge_BatteryFull", 67)]));
        assert_eq!(s.devices[1].status, Status::Discharging);
        assert_eq!(s.devices[1].level, Some(67));
    }

    #[test]
    fn older_events_are_ignored() {
        let mut s = State::default();
        s.apply(&snap("2026/10/09 10:00:00.000", &[dev(122, "NoCharge_BatteryFull", 50)]));
        s.apply(&snap("2026/10/09 09:00:00.000", &[dev(122, "NoCharge_BatteryFull", 90)]));
        assert_eq!(s.devices[2].level, Some(50));
    }

    #[test]
    fn low_uses_threshold() {
        let mut s = State::default();
        assert_eq!(s.devices[2].low(15), None);
        s.apply(&snap("2026/10/09 10:00:00.000", &[dev(122, "NoCharge_BatteryFull", 15)]));
        assert_eq!(s.devices[2].low(15), Some(false));
        assert_eq!(s.devices[2].low(20), Some(true));
        s.apply(&snap("2026/10/09 10:10:00.000", &[dev(122, "Charging", 5)]));
        assert_eq!(s.devices[2].low(15), Some(false));
        s.apply(&snap("2026/10/09 10:20:00.000", &[dev(122, "off", 0)]));
        assert_eq!(s.devices[2].low(15), None);
    }
}
