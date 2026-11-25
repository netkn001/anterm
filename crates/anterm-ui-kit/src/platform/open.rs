//! OS file opener & file-manager reveal helpers (`open`, `explorer`, `xdg-open`).

use std::io;
use std::path::{Path, PathBuf};

/// Normalize a path to the native OS spelling expected by system file openers.
///
/// On Windows, Explorer treats forward-slashed `c:/a.png` as a request to open
/// Documents rather than the file, so separators are normalized to `\` and an
/// ASCII drive letter is uppercased.
fn native_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(text) = path.to_str() {
            let mut out = text.replace('/', "\\");
            let bytes = out.as_bytes();
            if bytes.len() >= 2 && bytes[0].is_ascii_lowercase() && bytes[1] == b':' {
                let upper = (bytes[0] as char).to_ascii_uppercase().to_string();
                out.replace_range(..1, &upper);
            }
            return PathBuf::from(out);
        }
    }
    path.to_path_buf()
}

/// Hands a path to whatever the OS has it associated with. Also the fallback
/// for a directory the file tree cannot reach.
pub fn open_file_path(path: &Path) -> io::Result<()> {
    let path = native_path(path);
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener).arg(path).spawn()?;
    Ok(())
}

/// Shows a path where it lives, rather than opening it.
///
/// `open -R` and `explorer /select,` both select the file inside its folder;
/// no desktop-neutral Linux equivalent exists, so there the folder is opened
/// and the file is left for the eye to find.
pub fn reveal_file_path(path: &Path) -> io::Result<()> {
    let path = native_path(path);
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(&path);
        c
    };
    // Explorer wants `/select,` bare and the path quoted behind it. `arg`
    // quotes the whole thing the moment the path holds a space, and Explorer
    // answers a quoted switch by opening Documents and reporting success —
    // so the command line is written out by hand.
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("explorer");
        c.raw_arg(format!("/select,\"{}\"", path.display()));
        c
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(&path));
        c
    };
    command.spawn()?;
    Ok(())
}
