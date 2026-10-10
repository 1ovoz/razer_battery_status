//! Windows 실행 파일에 app.manifest와 버전 정보(파일 속성 → 자세히)를 넣는다.
//! (Windows 10/11 호환 선언과 모니터별 DPI 인식 설정)

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=Cargo.toml"); // 버전이 바뀌면 버전 리소스도 다시
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let manifest = dir.join("app.manifest");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
        return;
    }
    // GNU: windres로 리소스 오브젝트(매니페스트 + 버전 정보)를 만들어 링크
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rc = out.join("resources.rc");
    let obj = out.join("resources.o");
    let ver = env::var("CARGO_PKG_VERSION").unwrap();
    let mut nums: Vec<u32> = ver.split(['.', '-']).filter_map(|p| p.parse().ok()).collect();
    nums.resize(4, 0);
    let commas = nums.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(",");
    let rc_text = format!(
        r#"1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {commas}
PRODUCTVERSION {commas}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "Razer Battery Tray"
      VALUE "FileVersion", "{ver}"
      VALUE "ProductName", "Razer Battery Tray"
      VALUE "ProductVersion", "{ver}"
      VALUE "OriginalFilename", "razer_battery_tray.exe"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        manifest = manifest.display().to_string().replace('\\', "/"),
    );
    std::fs::write(&rc, rc_text).unwrap();
    let windres = env::var("WINDRES").unwrap_or_else(|_| {
        if cfg!(windows) { "windres".into() } else { "x86_64-w64-mingw32-windres".into() }
    });
    let status = Command::new(&windres)
        .args(["-O", "coff", "-i"]).arg(&rc).arg("-o").arg(&obj)
        .status()
        .unwrap_or_else(|e| panic!("{windres} 실행 실패: {e}"));
    assert!(status.success(), "windres 실패");
    println!("cargo:rustc-link-arg-bins={}", obj.display());
}
