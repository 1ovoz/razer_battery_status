//! 실제 Razer Synapse 로그 폴더로 하는 테스트.
//! 폴더 위치: RAZER_LOG_DIR 환경 변수 → %LOCALAPPDATA% → (WSL) /mnt/c/Users/*/AppData/Local/...

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use razer_battery_tray::parser::{State, Status, TRACKED};
use razer_battery_tray::watcher::{default_log_dir, Watcher};

const SUB: &str = "AppData/Local/Razer/RazerAppEngine/User Data/Logs";

fn log_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("RAZER_LOG_DIR") {
        return Some(d.into());
    }
    if let Some(d) = default_log_dir().filter(|d| d.is_dir()) {
        return Some(d);
    }
    fs::read_dir("/mnt/c/Users").ok()?.filter_map(|e| e.ok()).map(|e| e.path().join(SUB)).find(|p| p.is_dir())
}

fn real_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy().to_ascii_lowercase();
            n.starts_with("systray_systrayv2") && n.ends_with(".log")
        })
        .collect();
    v.sort();
    v
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("razer_tray_test_{}_{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn summary(s: &State) -> Vec<(Status, Option<u8>, Option<String>)> {
    s.devices.iter().map(|d| (d.status, d.level, d.level_ts.map(|t| String::from_utf8_lossy(&t).into_owned()))).collect()
}

#[test]
fn real_logs_parse_cleanly() {
    let Some(dir) = log_dir() else {
        eprintln!("로그 폴더를 찾지 못해 건너뜀");
        return;
    };
    let mut w = Watcher::new(&dir);
    let events = w.poll().unwrap();
    let st = w.stats;
    eprintln!("dir={} {:?}", dir.display(), st);
    assert!(st.files > 0, "systray_systrayv2*.log 없음");
    assert!(st.snapshots > 0, "기기 목록 스냅샷이 하나도 없음");
    assert_eq!(st.malformed, 0, "JSON 해석 실패 줄이 있음");
    assert!(events.windows(2).all(|p| p[0].ts <= p[1].ts), "정렬 안 됨");

    let mut state = State::default();
    state.apply_all(&events);
    for d in &state.devices {
        eprintln!("{:>5} {:?} {:?} -> {:?}", d.product_id, d.status, d.level, d.tooltip(d.name));
    }
    // 로그에 한 번이라도 잔량이 찍힌 기기는 Unknown이면 안 됨
    for (i, pid) in TRACKED.iter().map(|t| t.product_id).enumerate() {
        if events.iter().any(|e| matches!(e.obs[i], Some(razer_battery_tray::parser::Obs::On { .. }))) {
            assert_ne!(state.devices[i].status, Status::Unknown, "pid {pid}");
            assert!(state.devices[i].level.is_some());
        }
    }

    // 두 번째 poll: 그 사이 Synapse가 새로 쓴 줄만 읽어야 함 (보통 0~수 줄)
    let before = w.stats.bytes;
    let again = w.poll().unwrap();
    eprintln!("re-poll: {} new snapshots, {} new bytes", again.len(), w.stats.bytes - before);
    assert!(w.stats.bytes - before < 1024 * 1024);
}

/// 실제 로그를 여러 파일로 나눠 섞고(로테이션 흉내), 줄 중간에서 끊기는 조각 단위로
/// 이어 쓰면서 30초 poll을 흉내 낸다. 최종 상태가 한 번에 읽은 결과와 같아야 한다.
#[test]
fn incremental_matches_full_load() {
    let Some(dir) = log_dir() else { return };
    let mut full = State::default();
    let mut w = Watcher::new(&dir);
    full.apply_all(&w.poll().unwrap());
    let frozen_bytes = w.stats.bytes;

    // 테스트 도중 Synapse가 쓴 내용은 빼고 처음 읽은 만큼만 사용
    let mut data = Vec::new();
    for p in real_files(&dir) {
        data.extend_from_slice(&fs::read(&p).unwrap());
    }
    data.truncate(frozen_bytes.min(data.len() as u64) as usize);
    if let Some(i) = data.iter().rposition(|&b| b == b'\n') {
        data.truncate(i + 1);
    }
    let lines: Vec<&[u8]> = data.split_inclusive(|&b| b == b'\n').collect();

    // 파일 3개에 번갈아 기록
    let names = ["systray_systrayv2.log", "systray_systrayv21.log", "systray_systrayv22.log"];
    let mut per_file: Vec<Vec<u8>> = vec![Vec::new(); names.len()];
    for (i, l) in lines.iter().enumerate() {
        per_file[i % 3].extend_from_slice(l);
    }

    let tmp = temp_dir("incr");
    let mut handles: Vec<_> = names.iter().map(|n| fs::File::create(tmp.join(n)).unwrap()).collect();
    let mut written = vec![0usize; names.len()];
    let mut w2 = Watcher::new(&tmp);
    let mut incr = State::default();
    let mut step = 0usize;
    loop {
        let mut progressed = false;
        for f in 0..names.len() {
            // 일부러 줄 중간에서 끊기도록 소수 크기로 자름
            let chunk = 7_919 + (step * 104_729) % 300_007;
            let end = (written[f] + chunk).min(per_file[f].len());
            if end > written[f] {
                handles[f].write_all(&per_file[f][written[f]..end]).unwrap();
                handles[f].flush().unwrap();
                written[f] = end;
                progressed = true;
            }
        }
        incr.apply_all(&w2.poll().unwrap());
        step += 1;
        if !progressed {
            break;
        }
    }
    assert_eq!(w2.stats.malformed, 0);
    assert_eq!(summary(&incr), summary(&full));
    let _ = fs::remove_dir_all(&tmp);
}

/// 줄바꿈 없이 아주 긴 줄은 건너뛰고, 그 뒤의 정상 줄은 읽어야 한다
#[test]
fn overlong_line_is_skipped() {
    let tmp = temp_dir("long");
    let p = tmp.join("systray_systrayv2.log");
    let ok = r#"[2026/10/10 10:00:01.000] info: Device  [{"productId":122,"powerStatus":{"chargingStatus":"NoCharge_BatteryFull","level":42}}]"#;
    let mut data = format!("[2026/10/10 10:00:00.000] info: Device  [{}", "x".repeat(razer_battery_tray::watcher::MAX_LINE * 2));
    data += "\r\n";
    data += ok;
    data += "\r\n";
    fs::write(&p, data).unwrap();
    let mut w = Watcher::new(&tmp);
    let mut s = State::default();
    s.apply_all(&w.poll().unwrap());
    assert_eq!(w.stats.too_long, 1);
    assert_eq!(s.devices[2].level, Some(42));
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn shrunk_file_is_reread() {
    let tmp = temp_dir("shrink");
    let p = tmp.join("systray_systrayv2.log");
    let dev = |lvl: u32| format!(r#"[{{"productId":122,"powerStatus":{{"chargingStatus":"NoCharge_BatteryFull","level":{lvl}}}}}]"#);
    let pad = "x".repeat(500);
    fs::write(&p, format!("[2026/10/10 10:00:00.000] info: [{pad}]\r\n[2026/10/10 10:00:01.000] info: Device  {}\r\n[2026/10/10 10:00:02.000] info: Device  {}", dev(50), dev(49))).unwrap();
    let mut w = Watcher::new(&tmp);
    let mut s = State::default();
    s.apply_all(&w.poll().unwrap());
    assert_eq!(s.devices[2].level, Some(50), "덜 써진 마지막 줄은 처리하면 안 됨");

    // 마지막 줄 완성
    let mut f = fs::OpenOptions::new().append(true).open(&p).unwrap();
    f.write_all(b"\r\n").unwrap();
    drop(f);
    s.apply_all(&w.poll().unwrap());
    assert_eq!(s.devices[2].level, Some(49));

    // 로테이션: 더 작은 새 파일로 교체 → 처음부터 다시 읽음
    fs::write(&p, format!("[2026/10/10 11:00:00.000] info: Device  {}\r\n", dev(48))).unwrap();
    s.apply_all(&w.poll().unwrap());
    assert_eq!(s.devices[2].level, Some(48));
    let _ = fs::remove_dir_all(&tmp);
}
