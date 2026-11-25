//! The Linux half serves exactly one installation shape: an AppImage. The
//! installed artifact is a single file (the path `$APPIMAGE` names), so the
//! whole install is one atomic swap — move the running image aside, rename
//! the verified download into its place, and start it. Tarball and distro
//! installs never reach this program; `package_for_current_install` in
//! `core::update` hands them the release page instead.
//!
//! One Linux-specific constraint shapes the code: the image the GUI runs
//! from is a FUSE mount the AppImage runtime tears down when the app exits —
//! which is the moment `install` starts working. The GUI therefore copies
//! this helper out of the mount into the staging directory and runs the
//! copy, the same way the Windows path runs a private copy because Setup
//! replaces the installed one.

use std::fs;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anterm_protocol::updater::contract::{CMD_INSTALL, CMD_VERIFY};

use crate::common::{
    UnixTailOptions as TailOptions, healthy_after_grace, log_line, report_outcome, run_checked,
    swap_and_relaunch, verify_archive, wait_for_exit,
};

/// Where `bundle-appimage.sh` installs the desktop entry inside the
/// image. The root-level `anterm.desktop` is linuxdeploy's symlink to this
/// file, and extracting a symlink alone yields a dangling link — so the
/// real path is the one asked for.
const DESKTOP_ENTRY: &str = "usr/share/applications/anterm.desktop";
/// The helper inside the image, beside the app the way every platform
/// ships it.
const BUNDLED_UPDATER: &str = "usr/bin/anterm-updater";
/// The desktop-entry key `bundle-appimage.sh` stamps the release version
/// into — the AppImage convention for stating a version where tools can
/// read it without running the app.
const VERSION_KEY: &str = "X-AppImage-Version=";

pub fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let command = args
        .next()
        .and_then(|arg| arg.into_string().ok())
        .ok_or_else(usage)?;
    match command.as_str() {
        CMD_VERIFY => {
            let archive = next_path(&mut args)?;
            let checksums = next_path(&mut args)?;
            let asset_name = next_string(&mut args)?;
            let stage = next_path(&mut args)?;
            let expected_version = next_string(&mut args)?;
            reject_extra(args)?;
            verify_archive(&archive, &checksums, &asset_name)?;
            verify_update(&archive, &stage, &expected_version)
        }
        CMD_INSTALL => {
            let parent_pid = next_string(&mut args)?
                .parse::<u32>()
                .map_err(|_| "parent pid is not an unsigned integer".to_string())?;
            let current = next_path(&mut args)?;
            let archive = next_path(&mut args)?;
            let checksums = next_path(&mut args)?;
            let asset_name = next_string(&mut args)?;
            let stage = next_path(&mut args)?;
            let expected_version = next_string(&mut args)?;
            let log = next_path(&mut args)?;
            let options = tail_options(args)?;
            options.apply();
            install(InstallPlan {
                parent_pid,
                current,
                archive,
                checksums,
                asset_name,
                stage,
                expected_version,
                log,
                result_file: options.result_file,
            })
        }
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "usage: anterm-updater verify <archive.AppImage> <checksums.txt> \
     <asset-name> <stage-dir> <version>\n\
     or: anterm-updater install <parent-pid> <current.AppImage> <archive.AppImage> \
     <checksums.txt> <asset-name> <stage-dir> <version> <log-path> \
     [--config-dir <dir>] [--result-file <path>]"
        .to_string()
}

fn next_path(args: &mut impl Iterator<Item = std::ffi::OsString>) -> Result<PathBuf, String> {
    crate::common::next_path(args, usage)
}

fn next_string(args: &mut impl Iterator<Item = std::ffi::OsString>) -> Result<String, String> {
    crate::common::next_string(args, usage)
}

fn reject_extra(args: impl Iterator<Item = std::ffi::OsString>) -> Result<(), String> {
    crate::common::reject_extra(args, usage)
}

fn tail_options(args: impl Iterator<Item = std::ffi::OsString>) -> Result<TailOptions, String> {
    TailOptions::parse(args, usage)
}

struct InstallPlan {
    parent_pid: u32,
    current: PathBuf,
    archive: PathBuf,
    checksums: PathBuf,
    asset_name: String,
    stage: PathBuf,
    expected_version: String,
    log: PathBuf,
    result_file: Option<PathBuf>,
}

fn install(plan: InstallPlan) -> Result<(), String> {
    install_inner(&plan)
}

// The daemon is deliberately left running, exactly as on macOS: nothing
// locks a running executable's file on Linux, and the daemon serves its
// panes from the old mount until the user chooses to restart it — that
// is what keeps their shells alive across the update.
fn install_inner(plan: &InstallPlan) -> Result<(), String> {
    wait_for_exit(plan.parent_pid);
    log_line(&plan.log, "re-verifying the staged anterm update");
    let verification = verify_archive(&plan.archive, &plan.checksums, &plan.asset_name)
        .and_then(|()| verify_update(&plan.archive, &plan.stage, &plan.expected_version));
    if let Err(error) = verification {
        log_line(&plan.log, &error);
        let _ = fs::remove_dir_all(&plan.stage);
        let result = Err(error);
        // The outcome lands before the old app does: the relaunched GUI
        // merges it at startup, and a write afterward races that merge
        // (#540).
        report_outcome(
            plan.result_file.as_deref(),
            &plan.log,
            &plan.expected_version,
            &result,
        );
        let _ = launch_app(&plan.current);
        return result;
    }
    log_line(&plan.log, &format!("replacing {}", plan.current.display()));
    let report = |result: &Result<(), String>| {
        report_outcome(
            plan.result_file.as_deref(),
            &plan.log,
            &plan.expected_version,
            result,
        );
    };
    replace_and_relaunch(
        &plan.current,
        &plan.archive,
        &plan.stage,
        launch_app,
        report,
    )
    .inspect_err(|error| log_line(&plan.log, error))
}

/// What the downloaded file has to prove before it may become the
/// installation: it is a type-2 AppImage at all, it states the version
/// this update was for, and it carries its own updater — an image
/// without one would install fine and then be the last version that
/// ever could. Runs only after `verify_archive` has pinned the bytes to
/// the release's checksums.txt; from there, running the image's own
/// `--appimage-extract` is running the released code, which is exactly
/// what the swap is about to do anyway.
fn verify_update(staged: &Path, stage: &Path, expected_version: &str) -> Result<(), String> {
    if !is_appimage(&read_header(staged)?) {
        return Err(format!("{} is not a type-2 AppImage", staged.display()));
    }
    // Downloaded bytes land without the execute bit; extraction needs the
    // runtime to run. The definitive mode is set again at swap time, taken
    // from the file being replaced.
    make_executable(staged)?;
    let desktop = extract_entry(staged, stage, DESKTOP_ENTRY)?;
    let text = fs::read_to_string(&desktop)
        .map_err(|error| format!("reading {}: {error}", desktop.display()))?;
    let actual = version_from_desktop_entry(&text).ok_or_else(|| {
        format!("the staged AppImage's desktop entry carries no {VERSION_KEY}")
    })?;
    if actual != expected_version {
        return Err(format!(
            "the staged AppImage reports version {actual}, expected {expected_version}"
        ));
    }
    extract_entry(staged, stage, BUNDLED_UPDATER)?;
    Ok(())
}

/// ELF with the AppImage type-2 marker (`AI\x02` at offset 8). The
/// runtime the swap is about to spawn only exists behind this shape; a
/// wrongly published asset — a tarball under the AppImage name, an HTML
/// error page — fails here with a name instead of at launch.
fn is_appimage(header: &[u8]) -> bool {
    header.len() >= 11
        && header[..4] == [0x7f, b'E', b'L', b'F']
        && header[8..11] == [b'A', b'I', 0x02]
}

fn read_header(path: &Path) -> Result<Vec<u8>, String> {
    let mut file =
        fs::File::open(path).map_err(|error| format!("reading {}: {error}", path.display()))?;
    let mut header = [0u8; 16];
    let read = file
        .read(&mut header)
        .map_err(|error| format!("reading {}: {error}", path.display()))?;
    Ok(header[..read].to_vec())
}

fn make_executable(path: &Path) -> Result<(), String> {
    let mode = fs::metadata(path)
        .map_err(|error| format!("reading the mode of {}: {error}", path.display()))?
        .permissions()
        .mode();
    fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o755))
        .map_err(|error| format!("marking {} executable: {error}", path.display()))
}

/// Unpacks one entry of the staged image into `<stage>/squashfs-root/`
/// and returns the extracted file's path.
///
/// `--appimage-extract` is answered by the AppImage runtime itself,
/// before any application code, and unpacks without mounting — so it
/// works on machines whose FUSE setup the eventual launch will need but
/// this verification should not. Run from the staging directory so the
/// `squashfs-root` it creates lands beside the package and is removed
/// with it. The runtime exits zero even when nothing matched, which is
/// why the answer is the extracted file's existence rather than the
/// exit status.
fn extract_entry(appimage: &Path, stage: &Path, entry: &str) -> Result<PathBuf, String> {
    // Anchored before the spawn: exec resolves a relative program path
    // against the child's working directory, which the line below moves —
    // a hand-run `anterm-updater verify ./pkg.AppImage …` would otherwise
    // fail with a bare "No such file or directory".
    let appimage = std::path::absolute(appimage)
        .map_err(|error| format!("resolving {}: {error}", appimage.display()))?;
    run_checked(
        Command::new(&appimage)
            .args(["--appimage-extract", entry])
            .current_dir(stage)
            .stdout(Stdio::null()),
        "extracting from the staged AppImage",
    )?;
    let extracted = stage.join("squashfs-root").join(entry);
    if !extracted.is_file() {
        return Err(format!("the staged AppImage carries no {entry}"));
    }
    Ok(extracted)
}

fn version_from_desktop_entry(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix(VERSION_KEY))
        .map(|version| version.trim().to_string())
        .filter(|version| !version.is_empty())
}

fn replace_and_relaunch(
    current: &Path,
    replacement: &Path,
    stage: &Path,
    launch: impl Fn(&Path) -> Result<(), String>,
    report: impl Fn(&Result<(), String>),
) -> Result<(), String> {
    swap_and_relaunch(
        current,
        replacement,
        stage,
        "previous.AppImage",
        "AppImage",
        carry_mode,
        launch,
        report,
    )
}

/// Puts the mode of the file being replaced onto its replacement,
/// with owner execute assured. Falls back to plain 0o755 when the
/// current image cannot answer — it is about to be renamed away, not
/// consulted as an authority.
fn carry_mode(current: &Path, replacement: &Path) -> Result<(), String> {
    let mode = fs::metadata(current)
        .map(|meta| meta.permissions().mode())
        .unwrap_or(0o755);
    fs::set_permissions(replacement, fs::Permissions::from_mode(mode | 0o700))
        .map_err(|error| format!("setting the mode of {}: {error}", replacement.display()))
}

/// Starts the image at its installed path. The runtime sets `$APPIMAGE`
/// and `$APPDIR` for the process it mounts, overwriting the stale pair
/// this process inherited from the app that spawned it.
fn launch_app(appimage: &Path) -> Result<(), String> {
    let mut child = Command::new(appimage)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("launching {}: {error}", appimage.display()))?;
    healthy_after_grace(&mut child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_launch_commits_the_replacement() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("anterm.AppImage");
        let stage = root.path().join("stage");
        fs::create_dir(&stage).unwrap();
        let replacement = stage.join("anterm-new.AppImage");
        fs::write(&current, b"old image").unwrap();
        fs::write(&replacement, b"new image").unwrap();

        replace_and_relaunch(&current, &replacement, &stage, |_| Ok(()), |_| ()).unwrap();

        assert_eq!(fs::read(&current).unwrap(), b"new image");
        assert!(!stage.exists());
    }

    #[test]
    fn failed_launch_restores_and_relaunches_the_previous_image() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("anterm.AppImage");
        let stage = root.path().join("stage");
        fs::create_dir(&stage).unwrap();
        let replacement = stage.join("anterm-new.AppImage");
        fs::write(&current, b"old image").unwrap();
        fs::write(&replacement, b"new image").unwrap();
        let launches = std::cell::Cell::new(0);
        let reported_after_launches = std::cell::Cell::new(usize::MAX);

        let error = replace_and_relaunch(
            &current,
            &replacement,
            &stage,
            |_| {
                launches.set(launches.get() + 1);
                if launches.get() == 1 {
                    Err("new app failed".to_string())
                } else {
                    Ok(())
                }
            },
            |_| reported_after_launches.set(launches.get()),
        )
        .unwrap_err();

        assert_eq!(error, "new app failed");
        assert_eq!(launches.get(), 2);
        // The outcome is reported after the failed first launch but before
        // the old app comes back — the relaunched GUI must find it already
        // on disk at startup (#540).
        assert_eq!(reported_after_launches.get(), 1);
        assert_eq!(fs::read(&current).unwrap(), b"old image");
        assert!(!stage.exists());
    }

    /// The installed image keeps the mode the user gave it — a 0700
    /// image stays private — while the download's missing execute bit
    /// never survives into the installation.
    #[test]
    fn the_replacement_wears_the_current_images_mode() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("anterm.AppImage");
        let stage = root.path().join("stage");
        fs::create_dir(&stage).unwrap();
        let replacement = stage.join("anterm-new.AppImage");
        fs::write(&current, b"old image").unwrap();
        fs::write(&replacement, b"new image").unwrap();
        fs::set_permissions(&current, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o644)).unwrap();

        replace_and_relaunch(&current, &replacement, &stage, |_| Ok(()), |_| ()).unwrap();

        let mode = fs::metadata(&current).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "mode {mode:o}");
    }

    /// A leftover backup means an earlier attempt stopped between its two
    /// renames; installing over it would overwrite the one copy of the
    /// previous version.
    #[test]
    fn an_existing_backup_stops_the_replacement() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("anterm.AppImage");
        let stage = root.path().join("stage");
        fs::create_dir(&stage).unwrap();
        let replacement = stage.join("anterm-new.AppImage");
        fs::write(&current, b"old image").unwrap();
        fs::write(&replacement, b"new image").unwrap();
        fs::write(stage.join("previous.AppImage"), b"earlier backup").unwrap();

        let error = replace_and_relaunch(
            &current,
            &replacement,
            &stage,
            |_| panic!("nothing may launch when the backup path is taken"),
            |_| (),
        )
        .unwrap_err();

        assert!(error.contains("backup already exists"), "{error}");
        assert_eq!(fs::read(&current).unwrap(), b"old image");
    }

    #[test]
    fn archive_verification_rejects_bytes_that_do_not_match_the_manifest() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("anterm.AppImage");
        let manifest = root.path().join("checksums.txt");
        fs::write(&archive, b"downloaded bytes").unwrap();
        fs::write(
            &manifest,
            format!(
                "{}  anterm.AppImage\n",
                anterm_protocol::updater::checksums::hex(
                    &anterm_protocol::updater::checksums::sha256(b"published bytes")
                )
            ),
        )
        .unwrap();

        let error = verify_archive(&archive, &manifest, "anterm.AppImage").unwrap_err();
        assert!(error.contains("failed sha256 verification"), "{error}");
    }

    /// The magic check runs before anything executes the download, so a
    /// mis-published asset is named without being run.
    #[test]
    fn verification_rejects_a_file_that_is_not_an_appimage() {
        let mut elf_with_marker = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0, b'A', b'I', 0x02];
        elf_with_marker.resize(16, 0);
        assert!(is_appimage(&elf_with_marker));
        // A plain ELF — the tarball's binary, say — is not an AppImage.
        let mut bare_elf = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0];
        bare_elf.resize(16, 0);
        assert!(!is_appimage(&bare_elf));
        assert!(!is_appimage(b"<html>Not Found</html>"));
        assert!(!is_appimage(b""));
        assert!(!is_appimage(&[0x7f, b'E', b'L', b'F']));

        let root = tempfile::tempdir().unwrap();
        let staged = root.path().join("anterm.AppImage");
        fs::write(&staged, b"<html>Not Found</html>").unwrap();
        let error = verify_update(&staged, root.path(), "27.1.0").unwrap_err();
        assert!(error.contains("not a type-2 AppImage"), "{error}");
    }

    #[test]
    fn the_desktop_entry_states_the_version() {
        let text = "[Desktop Entry]\nType=Application\nName=anterm\nExec=anterm-app\n\
                    Icon=anterm\nX-AppImage-Version=26.8.4\n";
        assert_eq!(version_from_desktop_entry(text).as_deref(), Some("26.8.4"));
        // The nightly stamp survives whole — the identity the GUI
        // compares against carries the prerelease tail.
        assert_eq!(
            version_from_desktop_entry("X-AppImage-Version=26.8.4-nightly.202608140200\n")
                .as_deref(),
            Some("26.8.4-nightly.202608140200")
        );
        assert_eq!(
            version_from_desktop_entry("[Desktop Entry]\nName=anterm\n"),
            None
        );
        // A stated nothing is not a version.
        assert_eq!(version_from_desktop_entry("X-AppImage-Version=\n"), None);
        assert_eq!(version_from_desktop_entry("X-AppImage-Version=  \n"), None);
    }
}
