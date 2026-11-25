#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
#![cfg_attr(target_os = "windows", allow(unknown_lints, linker_messages))]
#![cfg_attr(
    not(any(target_os = "macos", target_os = "windows", target_os = "linux")),
    allow(dead_code)
)]

mod common;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
fn main() {
    if let Err(error) = macos::run() {
        eprintln!("anterm-updater: {error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = linux::run() {
        eprintln!("anterm-updater: {error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "windows")]
fn main() {
    if let Err(error) = windows::run() {
        eprintln!("anterm-updater: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn main() {
    eprintln!("anterm-updater is only used on macOS, Linux, and Windows");
    std::process::exit(1);
}
