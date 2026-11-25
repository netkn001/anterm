#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
// Non-English MSVC link.exe emits localized "Creating library ... and object ..."
// stdout lines when dependencies (such as aws-lc-sys) export symbols, which rustc's
// English-only linker-info filter does not recognize.
#![cfg_attr(target_os = "windows", allow(unknown_lints, linker_messages))]

fn main() {
    anterm_app::run();
}
