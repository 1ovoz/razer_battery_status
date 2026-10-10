//! 자동 업데이트: GitHub의 dist/latest.txt를 확인하고, 서명이 맞는 새 exe만 받아 둔다.
//!
//! - 통신은 WinHTTP, 해시·서명 검증은 BCrypt(Windows 기본 암호화). WinHTTP는 한 번 불러오면
//!   프로세스에서 내려가지 않으므로, 확인은 같은 exe를 `--update-check`로 띄운 짧은 프로세스에서
//!   하고 결과만 파일로 넘긴다(메인 앱의 메모리는 그대로).
//! - 서명 검증: tools/release.sh가 개발 PC의 비밀 키로 "버전\nSHA256\n"에 서명한다.
//!   앱에는 공개 키만 들어 있어서, GitHub 저장소가 털려도 그 키 없이는 가짜 exe를 배포할 수 없다.
//! - 실제 교체·재시작은 메인 앱(tray.rs)이 한다.

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

use razer_battery_tray::update_info::{is_newer, parse_latest};

use crate::tray::wide;

/// 업데이트 파일 위치. RBT_UPDATE_BASE 환경 변수로 바꿀 수 있다(로컬 테스트용).
const UPDATE_BASE: &str = "https://raw.githubusercontent.com/1ovoz/razer_battery_status/main/dist/";

/// 서명 확인용 ECDSA P-256 공개 키 (X || Y). 비밀 키는 개발 PC에만 있다.
const PUBLIC_KEY: &str = "5980f6911fee019c153f312082c63c4d108f34624f8bfe12b75ad81b51b1ab2b39ad3b8e9bd2916044fd5aa85c4dfe604834d9fa7c3c03c4d1a96cb4218a708c";

const MAX_LATEST: usize = 4 * 1024;
const MAX_EXE: usize = 16 * 1024 * 1024;

pub enum Outcome {
    UpToDate,
    /// 검증을 마친 새 exe가 path에 저장됨
    Ready { version: String, path: PathBuf },
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

// ───────────── WinHTTP ─────────────

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
            let mut buf = vec![0u8; 16 * 1024];
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

// ───────────── BCrypt (SHA-256, ECDSA P-256) ─────────────

struct Crypto {
    _dll: Dll,
    open_alg: unsafe extern "system" fn(*mut H, *const u16, *const u16, u32) -> i32,
    close_alg: unsafe extern "system" fn(H, u32) -> i32,
    hash: unsafe extern "system" fn(H, *const u8, u32, *const u8, u32, *mut u8, u32) -> i32,
    import_key: unsafe extern "system" fn(H, H, *const u16, *mut H, *const u8, u32, u32) -> i32,
    verify: unsafe extern "system" fn(H, *const c_void, *const u8, u32, *const u8, u32, u32) -> i32,
    destroy_key: unsafe extern "system" fn(H) -> i32,
}

impl Crypto {
    fn load() -> Result<Self, String> {
        let dll = Dll::load("bcrypt.dll")?;
        unsafe {
            Ok(Crypto {
                open_alg: dll.get(b"BCryptOpenAlgorithmProvider\0")?,
                close_alg: dll.get(b"BCryptCloseAlgorithmProvider\0")?,
                hash: dll.get(b"BCryptHash\0")?,
                import_key: dll.get(b"BCryptImportKeyPair\0")?,
                verify: dll.get(b"BCryptVerifySignature\0")?,
                destroy_key: dll.get(b"BCryptDestroyKey\0")?,
                _dll: dll,
            })
        }
    }

    fn sha256(&self, data: &[u8]) -> Result<[u8; 32], String> {
        unsafe {
            let mut alg: H = null_mut();
            if (self.open_alg)(&mut alg, wide("SHA256").as_ptr(), null(), 0) != 0 {
                return Err("SHA256 준비 실패".into());
            }
            let mut out = [0u8; 32];
            let st = (self.hash)(alg, null(), 0, data.as_ptr(), data.len() as u32, out.as_mut_ptr(), 32);
            (self.close_alg)(alg, 0);
            if st != 0 { Err("SHA256 계산 실패".into()) } else { Ok(out) }
        }
    }

    /// message의 SHA-256에 대한 ECDSA P-256 서명(r||s)이 공개 키와 맞는지
    fn verify_p256(&self, public_xy: &[u8; 64], message: &[u8], signature: &[u8; 64]) -> Result<bool, String> {
        let digest = self.sha256(message)?;
        unsafe {
            let mut alg: H = null_mut();
            if (self.open_alg)(&mut alg, wide("ECDSA_P256").as_ptr(), null(), 0) != 0 {
                return Err("ECDSA 준비 실패".into());
            }
            // BCRYPT_ECCKEY_BLOB { magic = BCRYPT_ECDSA_PUBLIC_P256_MAGIC("ECS1"), cbKey = 32 } + X + Y
            let mut blob = Vec::with_capacity(8 + 64);
            blob.extend_from_slice(&0x3153_4345u32.to_le_bytes());
            blob.extend_from_slice(&32u32.to_le_bytes());
            blob.extend_from_slice(public_xy);
            let mut key: H = null_mut();
            let st = (self.import_key)(alg, null_mut(), wide("ECCPUBLICBLOB").as_ptr(), &mut key, blob.as_ptr(), blob.len() as u32, 0);
            if st != 0 {
                (self.close_alg)(alg, 0);
                return Err("공개 키 불러오기 실패".into());
            }
            let ok = (self.verify)(key, null(), digest.as_ptr(), 32, signature.as_ptr(), 64, 0) == 0;
            (self.destroy_key)(key);
            (self.close_alg)(alg, 0);
            Ok(ok)
        }
    }
}

fn public_key() -> [u8; 64] {
    let mut k = [0u8; 64];
    for (i, b) in k.iter_mut().enumerate() {
        *b = u8::from_str_radix(&PUBLIC_KEY[i * 2..i * 2 + 2], 16).unwrap_or(0);
    }
    k
}

/// %LOCALAPPDATA%\RazerBatteryTray\update
pub fn update_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("RazerBatteryTray").join("update"))
}

fn result_path() -> Option<PathBuf> {
    Some(update_dir()?.join("check_result.txt"))
}

/// 확인 전용 프로세스(`--update-check`)의 본체: 확인하고 결과를 파일에 쓴 뒤 끝난다
pub fn run_check_process(current: &str) {
    let text = match check_inner(current) {
        Ok(Outcome::UpToDate) => "UPTODATE".to_string(),
        Ok(Outcome::Ready { version, .. }) => format!("READY\n{version}"),
        Ok(Outcome::Failed(e)) | Err(e) => format!("FAILED\n{e}"),
    };
    if let Some(p) = result_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, text);
    }
}

/// 메인 앱에서: 확인 프로세스를 띄워 끝날 때까지(최대 2분) 기다리고 결과를 읽는다 (블로킹 → 스레드에서 호출)
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
            Ok(None) if waited < 240 => {
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
        (Some("READY"), Some(v)) => match update_dir() {
            Some(dir) => Outcome::Ready { version: v.to_string(), path: dir.join("razer_battery_tray.new.exe") },
            None => Outcome::Failed("LOCALAPPDATA 없음".into()),
        },
        (Some("FAILED"), msg) => Outcome::Failed(msg.unwrap_or("알 수 없는 오류").to_string()),
        _ => Outcome::Failed("확인 결과를 읽지 못함".into()),
    }
}

fn check_inner(current: &str) -> Result<Outcome, String> {
    let base = std::env::var("RBT_UPDATE_BASE").unwrap_or_else(|_| UPDATE_BASE.to_string());
    let http = Http::load()?;
    let text = http.get(&format!("{base}latest.txt"), MAX_LATEST)?;
    let latest = parse_latest(&String::from_utf8_lossy(&text)).ok_or("latest.txt 형식 오류")?;
    if !is_newer(&latest.version, current) {
        return Ok(Outcome::UpToDate);
    }

    let crypto = Crypto::load()?;
    // 1) 버전·해시 목록이 개발자 키로 서명됐는지
    if !crypto.verify_p256(&public_key(), latest.signed_message.as_bytes(), &latest.signature)? {
        return Err("서명이 맞지 않아 업데이트를 거부했습니다".into());
    }
    // 2) 받은 exe가 서명된 해시와 같은지
    let exe = http.get(&format!("{base}razer_battery_tray.exe"), MAX_EXE)?;
    if crypto.sha256(&exe)? != latest.sha256 {
        return Err("받은 파일이 서명된 내용과 달라 업데이트를 거부했습니다".into());
    }
    // 3) 검증된 파일만 저장
    let dir = update_dir().ok_or("LOCALAPPDATA 없음")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("razer_battery_tray.new.exe");
    std::fs::write(&path, &exe).map_err(|e| format!("저장 실패: {e}"))?;
    Ok(Outcome::Ready { version: latest.version, path })
}
