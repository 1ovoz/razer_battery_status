//! 로그 폴더의 systray_systrayv2*.log 파일들을 오프셋 기반으로 이어 읽는다.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::parser::{parse_line, Event, Line};

const PREFIX: &str = "systray_systrayv2";
/// 한 줄 최대 길이. 실제 줄은 수십 KB지만, 줄바꿈 없는 비정상 파일이 메모리를 잡아먹지 않게 자른다.
pub const MAX_LINE: usize = 1 << 20;
const SUFFIX: &str = ".log";

struct Cursor {
    name: OsString,
    offset: u64,
    created: Option<SystemTime>,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Stats {
    pub files: usize,
    pub bytes: u64,
    pub lines: usize,
    pub snapshots: usize,
    pub malformed: usize,
    /// MAX_LINE보다 길어 건너뛴 줄
    pub too_long: usize,
}

pub struct Watcher {
    dir: PathBuf,
    cursors: Vec<Cursor>,
    pub stats: Stats,
}

/// %LOCALAPPDATA%\Razer\RazerAppEngine\User Data\Logs
pub fn default_log_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(Path::new(&base).join(r"Razer\RazerAppEngine\User Data\Logs"))
}

impl Watcher {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), cursors: Vec::new(), stats: Stats::default() }
    }

    /// 모든 대상 파일에서 지난번 이후 새로 추가된 완결된 줄만 읽어
    /// 시각 순으로 정렬된 스냅샷 목록을 돌려준다. 처음 호출하면 전체를 읽는다.
    pub fn poll(&mut self) -> io::Result<Vec<Event>> {
        let mut names: Vec<OsString> = fs::read_dir(&self.dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .filter(|n| {
                n.to_str().is_some_and(|s| {
                    let l = s.to_ascii_lowercase();
                    l.starts_with(PREFIX) && l.ends_with(SUFFIX)
                })
            })
            .collect();
        names.sort();
        // 사라진 파일의 커서는 버린다
        self.cursors.retain(|c| names.contains(&c.name));
        self.stats.files = names.len();

        let mut events = Vec::new();
        let mut line = Vec::new();
        for name in names {
            let idx = match self.cursors.iter().position(|c| c.name == name) {
                Some(i) => i,
                None => {
                    self.cursors.push(Cursor { name, offset: 0, created: None });
                    self.cursors.len() - 1
                }
            };
            let path = self.dir.join(&self.cursors[idx].name);
            // 파일 하나가 실패해도(잠김 등) 나머지는 계속 읽는다
            let _ = read_new(&path, &mut self.cursors[idx], &mut line, &mut events, &mut self.stats);
        }
        // 여러 파일에 섞여 기록되므로 시각 순 정렬 (같은 시각은 원래 순서 유지)
        events.sort_by_key(|e| e.ts);
        Ok(events)
    }
}

fn read_new(
    path: &Path,
    cur: &mut Cursor,
    line: &mut Vec<u8>,
    out: &mut Vec<Event>,
    stats: &mut Stats,
) -> io::Result<()> {
    // std::fs::File::open은 Windows에서 FILE_SHARE_READ|WRITE|DELETE로 열기 때문에
    // Synapse가 파일을 쓰거나 교체하는 것을 막지 않는다.
    let file = File::open(path)?;
    let meta = file.metadata()?;
    let len = meta.len();
    let created = meta.created().ok();
    if len < cur.offset || (cur.created.is_some() && created != cur.created) {
        cur.offset = 0; // 교체(로테이션)됨 → 처음부터
    }
    cur.created = created;
    if len == cur.offset {
        return Ok(());
    }

    let mut reader = BufReader::with_capacity(64 * 1024, file);
    reader.seek(SeekFrom::Start(cur.offset))?;
    let mut reader = reader.take(len - cur.offset);
    let mut pos = cur.offset;
    loop {
        // 한 줄 읽기 (MAX_LINE을 넘는 부분은 버리고 줄 끝까지 건너뜀)
        line.clear();
        let (mut consumed, mut complete, mut too_long) = (0usize, false, false);
        loop {
            let buf = reader.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            let (take, found) = match buf.iter().position(|&b| b == b'\n') {
                Some(i) => (i + 1, true),
                None => (buf.len(), false),
            };
            if !too_long {
                if line.len() + take > MAX_LINE {
                    too_long = true;
                    line.clear();
                    line.shrink_to(64 * 1024);
                } else {
                    line.extend_from_slice(&buf[..take]);
                }
            }
            reader.consume(take);
            consumed += take;
            if found {
                complete = true;
                break;
            }
        }
        if !complete {
            break; // 끝까지 읽었거나 마지막 줄이 아직 덜 써짐 → 다음에 같은 자리부터 다시
        }
        pos += consumed as u64;
        stats.lines += 1;
        if too_long {
            stats.too_long += 1;
            continue;
        }
        match parse_line(line) {
            Line::Snapshot(e) => {
                stats.snapshots += 1;
                out.push(e);
            }
            Line::Malformed => stats.malformed += 1,
            Line::Irrelevant | Line::NotDeviceList => {}
        }
    }
    stats.bytes += pos - cur.offset;
    cur.offset = pos;
    Ok(())
}
