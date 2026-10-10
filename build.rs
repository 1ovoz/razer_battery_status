//! Windows 실행 파일에 app.manifest를 넣는다.
//! (Windows 10/11 호환 선언과 모니터별 DPI 인식 설정)

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
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
    // GNU: windres로 리소스 오브젝트를 만들어 링크
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rc = out.join("manifest.rc");
    let obj = out.join("manifest.o");
    std::fs::write(&rc, format!("1 24 \"{}\"\n", manifest.display().to_string().replace('\\', "/"))).unwrap();
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
