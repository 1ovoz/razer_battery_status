//! 새 버전 알림용 dist/version.txt 해석과 버전 비교 (Win32 의존성 없음 → 어디서나 테스트 가능).
//! version.txt는 버전 번호 한 줄이다 (tools/release.sh가 만든다).

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

/// version.txt 내용 → 버전 문자열 (첫 줄, 형식이 맞을 때만)
pub fn parse_version_file(text: &str) -> Option<String> {
    let v = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    parse_version(v)?;
    Some(v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("1.1.0", "1.0.0"));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2", "1.99"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(!is_newer("0.9.9", "1.0.0"));
        assert!(!is_newer("1.0.0-beta", "0.1.0"), "숫자가 아니면 새 버전으로 보지 않음");
        assert!(!is_newer("", "1.0.0"));
    }

    #[test]
    fn version_file() {
        assert_eq!(parse_version_file("1.2.0\r\n").as_deref(), Some("1.2.0"));
        assert_eq!(parse_version_file("\n  1.3.1  \n").as_deref(), Some("1.3.1"));
        assert_eq!(parse_version_file(""), None);
        assert_eq!(parse_version_file("<html>404</html>"), None);
        assert_eq!(parse_version_file("1.2.x"), None);
    }
}
