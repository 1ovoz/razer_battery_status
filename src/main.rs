#![windows_subsystem = "windows"]

#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod widget;

#[cfg(windows)]
fn main() {
    tray::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("이 프로그램은 Windows 전용입니다.");
}
