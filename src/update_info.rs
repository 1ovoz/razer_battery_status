//! 자동 업데이트용 dist/latest.txt 해석과 버전 비교 (Win32 의존성 없음 → 어디서나 테스트 가능).
//!
//! latest.txt 형식 (tools/release.sh가 만든다):
//! ```text
//! 1.1.0                      ← 버전
//! <exe SHA-256, 소문자 hex 64자>
//! <ECDSA P-256 서명 r||s, hex 128자>   ← "버전\nSHA256\n" 에 대한 서명
//! ```

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Latest {
    pub version: String,
    pub sha256: [u8; 32],
    pub signature: [u8; 64],
    /// 서명 검증에 쓰는 원문 ("버전\nSHA256\n")
    pub signed_message: String,
}

fn hex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let s = s.trim();
    if s.len() != N * 2 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// "1.2.3" → [1, 2, 3]. 숫자가 아닌 부분이 있으면 None.
pub fn parse_version(v: &str) -> Option<Vec<u32>> {
    let parts: Option<Vec<u32>> = v.trim().split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty() && p.len() <= 4)
}

/// candidate가 current보다 새 버전이면 true (1.10.0 > 1.9.0, 1.1 == 1.1.0)
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(mut a), Some(mut b)) => {
            let n = a.len().max(b.len());
            a.resize(n, 0);
            b.resize(n, 0);
            a > b
        }
        _ => false,
    }
}

pub fn parse_latest(text: &str) -> Option<Latest> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let version = lines.next()?.to_string();
    parse_version(&version)?;
    let sha_hex = lines.next()?.to_ascii_lowercase();
    let sha256 = hex::<32>(&sha_hex)?;
    let signature = hex::<64>(lines.next()?)?;
    Some(Latest { signed_message: format!("{version}\n{sha_hex}\n"), version, sha256, signature })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "e57323a7c3865273cd679dffc2f11d0853baa6b7c5d15421f14a318e33c60ce8";

    #[test]
    fn versions() {
        assert!(is_newer("1.1.0", "1.0.0"));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2", "1.99"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(!is_newer("0.9.9", "1.0.0"));
        assert!(!is_newer("1.0.0-beta", "0.1.0"), "숫자가 아니면 업데이트하지 않음");
        assert!(!is_newer("", "1.0.0"));
    }

    #[test]
    fn parses_latest() {
        let sig = "ab".repeat(64);
        let l = parse_latest(&format!("1.1.0\r\n{}\r\n{sig}\r\n", SHA.to_uppercase())).unwrap();
        assert_eq!(l.version, "1.1.0");
        assert_eq!(l.sha256[0], 0xe5);
        assert_eq!(l.signature, [0xab; 64]);
        assert_eq!(l.signed_message, format!("1.1.0\n{SHA}\n"));
    }

    #[test]
    fn rejects_bad_latest() {
        let sig = "ab".repeat(64);
        assert!(parse_latest("").is_none());
        assert!(parse_latest(&format!("x.y\n{SHA}\n{sig}")).is_none());
        assert!(parse_latest(&format!("1.1.0\n{}\n{sig}", &SHA[..60])).is_none());
        assert!(parse_latest(&format!("1.1.0\n{SHA}\n{}", &sig[..100])).is_none());
        assert!(parse_latest(&format!("1.1.0\n{SHA}")).is_none());
        assert!(parse_latest(&format!("1.1.0\n{}\n{sig}", "zz".repeat(32))).is_none());
    }
}
