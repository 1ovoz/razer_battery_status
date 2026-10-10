//! 새 버전 알림: GitHub의 dist/version.txt에서 최신 버전 번호만 확인한다.
//! 설치는 하지 않는다. 새 버전이 있으면 알림을 띄우고, 사용자가 직접 받아서 바꾼다.
//!
//! - 통신은 WinHTTP(Windows 기본). WinHTTP는 한 번 불러오면 프로세스에서 내려가지 않으므로,
//!   확인은 같은 exe를 `--update-check`로 띄운 짧은 프로세스에서 하고 결과만 파일로 넘긴다
//!   (메인 앱의 메모리는 그대로).

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

use razer_battery_tray::update_info::{is_newer, parse_version, parse_version_file};

use crate::tray::wide;

/// 최신 버전 번호 파일 위치. 로컬 테스트용으로 RBT_UPDATE_BASE 환경 변수로 바꿀 수 있지만 localhost만 허용한다.
const UPDATE_BASE: &str = "https://raw.githubusercontent.com/1ovoz/razer_battery_status/main/dist/";
/// 새 버전 알림을 누르면 여는 다운로드 페이지
pub const DOWNLOAD_PAGE: &str = "https://github.com/1ovoz/razer_battery_status#설치";

const MAX_VERSION_FILE: usize = 256;

pub enum Outcome {
    UpToDate,
    Newer(String),
    Failed(String),
}

/// System32에서만 불러온 DLL. 다 쓰면 내려놓는다.
struct Dll(HMODULE);

impl Dll {
    fn load(name: &str) -> Result<Self, String> {
        let h = unsafe { LoadLibraryExW(wide(name).as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        if h.is_null() { Err(format!("{name}을 불러오지 못했습니다")) } else { Ok(Dll(h)) }
    }

    /// 함수 주소를 원하는 함수 포인터 형식으로 가져온다
    unsafe fn get<F: Copy>(&self, name: &'static [u8]) -> Result<F, String> {
        let p = unsafe { GetProcAddress(self.0, name.as_ptr()) }
            .ok_or_else(|| format!("{} 없음", String::from_utf8_lossy(&name[..name.len() - 1])))?;
        assert_eq!(size_of::<F>(), size_of::<usize>());
        Ok(unsafe { std::mem::transmute_copy::<_, F>(&p) })
    }
}

impl Drop for Dll {
    fn drop(&mut self) {
        unsafe { FreeLibrary(self.0) };
    }
}

type H = *mut c_void;

struct Http {
    _dll: Dll,
    open: unsafe extern "system" fn(*const u16, u32, *const u16, *const u16, u32) -> H,
    connect: unsafe extern "system" fn(H, *const u16, u16, u32) -> H,
    open_request: unsafe extern "system" fn(H, *const u16, *const u16, *const u16, *const u16, *const *const u16, u32) -> H,
    send: unsafe extern "system" fn(H, *const u16, u32, *const c_void, u32, u32, usize) -> i32,
    receive: unsafe extern "system" fn(H, *mut c_void) -> i32,
    query_headers: unsafe extern "system" fn(H, u32, *const u16, *mut c_void, *mut u32, *mut u32) -> i32,
    read: unsafe extern "system" fn(H, *mut c_void, u32, *mut u32) -> i32,
    set_timeouts: unsafe extern "system" fn(H, i32, i32, i32, i32) -> i32,
    close: unsafe extern "system" fn(H) -> i32,
}

/// 닫는 걸 잊지 않게 핸들을 감싼다
struct Handle<'a>(H, &'a Http);

impl Drop for Handle<'_> {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { (self.1.close)(self.0) };
        }
    }
}

impl Http {
    fn load() -> Result<Self, String> {
        let dll = Dll::load("winhttp.dll")?;
        unsafe {
            Ok(Http {
                open: dll.get(b"WinHttpOpen\0")?,
                connect: dll.get(b"WinHttpConnect\0")?,
                open_request: dll.get(b"WinHttpOpenRequest\0")?,
                send: dll.get(b"WinHttpSendRequest\0")?,
                receive: dll.get(b"WinHttpReceiveResponse\0")?,
                query_headers: dll.get(b"WinHttpQueryHeaders\0")?,
                read: dll.get(b"WinHttpReadData\0")?,
                set_timeouts: dll.get(b"WinHttpSetTimeouts\0")?,
                close: dll.get(b"WinHttpCloseHandle\0")?,
                _dll: dll,
            })
        }
    }

    /// GET url → 본문 (max바이트까지)
    fn get(&self, url: &str, max: usize) -> Result<Vec<u8>, String> {
        let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("http://") {
            (false, r)
        } else {
            return Err("잘못된 주소".into());
        };
        let (host_port, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
        let (host, port) = match host_port.split_once(':') {
            Some((h, p)) => (h, p.parse().map_err(|_| "잘못된 포트")?),
            None => (host_port, if secure { 443 } else { 80 }),
        };
        unsafe {
            // 4 = WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY (시스템 프록시 설정을 따름)
            let ua = wide(concat!("RazerBatteryTray/", env!("CARGO_PKG_VERSION")));
            let session = Handle((self.open)(ua.as_ptr(), 4, null(), null(), 0), self);
            if session.0.is_null() {
                return Err("통신 준비 실패".into());
            }
            (self.set_timeouts)(session.0, 10_000, 10_000, 10_000, 30_000);
            let conn = Handle((self.connect)(session.0, wide(host).as_ptr(), port, 0), self);
            if conn.0.is_null() {
                return Err("서버 연결 실패".into());
            }
            let flags = if secure { 0x0080_0000 } else { 0 }; // WINHTTP_FLAG_SECURE
            let req = Handle(
                (self.open_request)(conn.0, wide("GET").as_ptr(), wide(&path).as_ptr(), null(), null(), null(), flags),
                self,
            );
            if req.0.is_null()
                || (self.send)(req.0, null(), 0, null(), 0, 0, 0) == 0
                || (self.receive)(req.0, null_mut()) == 0
            {
                return Err("요청 실패 (인터넷 연결 확인)".into());
            }
            let mut status: u32 = 0;
            let mut len = 4u32;
            // 19 = WINHTTP_QUERY_STATUS_CODE, 0x20000000 = WINHTTP_QUERY_FLAG_NUMBER
            (self.query_headers)(req.0, 19 | 0x2000_0000, null(), &mut status as *mut u32 as *mut _, &mut len, null_mut());
            if status != 200 {
                return Err(format!("서버 응답 {status}"));
            }
            let mut body = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let mut n = 0u32;
                if (self.read)(req.0, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut n) == 0 {
                    return Err("받는 중 끊김".into());
                }
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&buf[..n as usize]);
                if body.len() > max {
                    return Err("파일이 너무 큼".into());
                }
            }
            Ok(body)
        }
    }
}

/// %LOCALAPPDATA%\RazerBatteryTray
pub fn app_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("RazerBatteryTray"))
}

fn result_path() -> Option<PathBuf> {
    Some(app_dir()?.join("update_check.txt"))
}

fn check_inner(current: &str) -> Result<Outcome, String> {
    let base = match std::env::var("RBT_UPDATE_BASE") {
        Ok(b) if b.starts_with("http://localhost:") || b.starts_with("http://127.0.0.1:") => b,
        _ => UPDATE_BASE.to_string(),
    };
    let text = Http::load()?.get(&format!("{base}version.txt"), MAX_VERSION_FILE)?;
    let latest = parse_version_file(&String::from_utf8_lossy(&text)).ok_or("version.txt 형식 오류")?;
    Ok(if is_newer(&latest, current) { Outcome::Newer(latest) } else { Outcome::UpToDate })
}

/// 확인 전용 프로세스(`--update-check`)의 본체: 확인하고 결과를 파일에 쓴 뒤 끝난다
pub fn run_check_process(current: &str) {
    let text = match check_inner(current) {
        Ok(Outcome::UpToDate) => "UPTODATE".to_string(),
        Ok(Outcome::Newer(v)) => format!("NEWER\n{v}"),
        Ok(Outcome::Failed(e)) | Err(e) => format!("FAILED\n{e}"),
    };
    if let Some(p) = result_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, text);
    }
}

/// 메인 앱에서: 확인 프로세스를 띄워 끝날 때까지(최대 1분) 기다리고 결과를 읽는다 (블로킹 → 스레드에서 호출)
pub fn check_in_child() -> Outcome {
    let (Ok(exe), Some(result)) = (std::env::current_exe(), result_path()) else {
        return Outcome::Failed("실행 파일 위치를 알 수 없음".into());
    };
    let _ = std::fs::remove_file(&result);
    let mut child = match std::process::Command::new(&exe).arg("--update-check").spawn() {
        Ok(c) => c,
        Err(e) => return Outcome::Failed(format!("확인 프로세스 실행 실패: {e}")),
    };
    let mut waited = 0;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if waited < 120 => {
                std::thread::sleep(std::time::Duration::from_millis(500));
                waited += 1;
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Outcome::Failed("확인 시간 초과".into());
            }
        }
    }
    let text = std::fs::read_to_string(&result).unwrap_or_default();
    let _ = std::fs::remove_file(&result);
    let mut lines = text.lines();
    match (lines.next(), lines.next()) {
        (Some("UPTODATE"), _) => Outcome::UpToDate,
        // 결과 파일은 사용자 폴더에 있으니 여기서도 형식을 다시 확인한다 (알림·메뉴에 그대로 표시되므로)
        (Some("NEWER"), Some(v)) if parse_version(v).is_some() => Outcome::Newer(v.trim().to_string()),
        (Some("FAILED"), msg) => Outcome::Failed(msg.unwrap_or("알 수 없는 오류").to_string()),
        _ => Outcome::Failed("확인 결과를 읽지 못함".into()),
    }
}
