#[cfg(any(unix, windows))]
use std::io;
#[cfg(unix)]
use std::path::Path;
#[cfg(any(unix, windows))]
use std::path::PathBuf;
#[cfg(any(unix, windows))]
use std::sync::Arc;
#[cfg(unix)]
use std::sync::Mutex;

#[cfg(any(unix, windows))]
use crate::daemon::control::server_started;
#[cfg(any(unix, windows))]
use crate::host::SharedHost;

#[cfg(any(unix, windows))]
use super::{Services, serve_with};

pub const CONTROL_SOCK_ENV: &str = "ANTERM_CONTROL_SOCK";

#[cfg(unix)]
pub(super) mod sock {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::net::{UnixListener, UnixStream};

    const MAX_SOCKET_PATH_BYTES: usize = 100;

    /// The control endpoint, derived from the config dir exactly the way the
    /// pane endpoint is (`transport::socket_path_for`).
    ///
    /// It used to sit in `$XDG_RUNTIME_DIR/anterm` or `~/.local/share/anterm`,
    /// which meant it did not follow `--config-dir` while the pane socket did.
    /// Two consequences, both real: a `--config-dir` instance published the
    /// *default* control socket to the shells it spawned, so a CLI inside it
    /// talked to a different server entirely; and because both endpoints were
    /// named `daemon.sock`, telling them apart depended on their directories
    /// differing — so deriving one from the other by filename silently yielded
    /// the wrong socket. They are siblings now, distinguished by name, which is
    /// what the Windows arm has always done (`control.port`/`daemon.port`).
    pub fn control_socket_path() -> io::Result<PathBuf> {
        if let Some(explicit) = std::env::var_os(CONTROL_SOCK_ENV).filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(explicit));
        }

        let dir = crate::config::config_dir_path().ok_or_else(|| {
            io::Error::other("no config directory to place the control socket in")
        })?;

        let fallbacks: Vec<PathBuf> = [runtime_dir(), Some(std::env::temp_dir())]
            .into_iter()
            .flatten()
            .collect();
        socket_path_in(&dir, &fallbacks)
    }

    pub(in crate::host::server) const CONTROL_SOCK_FILE: &str = "control.sock";

    pub(crate) fn socket_path_in(dir: &Path, fallbacks: &[PathBuf]) -> io::Result<PathBuf> {
        let inline = dir.join(CONTROL_SOCK_FILE);
        if fits(&inline) {
            return Ok(inline);
        }

        use std::os::unix::ffi::OsStrExt as _;
        // `-control` keeps this clear of the pane socket's fallback name, which
        // hashes the same directory and would otherwise land on the same file.
        let name = format!(
            "anterm-{:016x}-control.sock",
            crate::host::fnv1a64(dir.as_os_str().as_bytes())
        );
        for base in fallbacks {
            let candidate = base.join(&name);
            if fits(&candidate) {
                return Ok(candidate);
            }
        }
        Err(io::Error::other(format!(
            "no directory short enough for a control socket ({MAX_SOCKET_PATH_BYTES}-byte \
             sun_path limit); set {CONTROL_SOCK_ENV} to one that is"
        )))
    }

    fn runtime_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
    }

    fn fits(p: &Path) -> bool {
        use std::os::unix::ffi::OsStrExt as _;
        p.as_os_str().as_bytes().len() <= MAX_SOCKET_PATH_BYTES
    }

    /// Whether a control server is answering at `path` right now.
    ///
    /// This, and not the errno a failed bind carries, is the question that
    /// matters to a daemon whose listener would not open. [`bind_control_socket`]
    /// does clear the ordinary leftover — it connects first and unlinks a socket
    /// nobody is behind — but what it cannot clear it hands back as `AddrInUse`
    /// all the same: a directory standing where the socket goes, a file owned by
    /// another user. Those look exactly like a live server in the error, and
    /// nothing is listening behind either. Only a connect that completes tells
    /// them apart.
    pub(crate) fn control_socket_answers(path: &Path) -> bool {
        UnixStream::connect(path).is_ok()
    }

    pub fn control_endpoint_answers() -> bool {
        control_socket_path().is_ok_and(|path| control_socket_answers(&path))
    }

    pub fn bind_control_socket(path: &Path) -> io::Result<UnixListener> {
        let parent = path.parent().unwrap_or(Path::new("."));
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }

        if path.exists() {
            match UnixStream::connect(path) {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        format!(
                            "a control server is already listening on {}",
                            path.display()
                        ),
                    ));
                }
                Err(_) => {
                    let _ = std::fs::remove_file(path);
                }
            }
        }

        let listener = bind_private(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    pub(in crate::host::server) fn bind_private(path: &Path) -> io::Result<UnixListener> {
        static UMASK: Mutex<()> = Mutex::new(());
        let _held = UMASK.lock().unwrap_or_else(|e| e.into_inner());

        let previous = unsafe { libc::umask(0o077) };
        let bound = UnixListener::bind(path);
        unsafe { libc::umask(previous) };
        bound
    }

    pub fn serve_listener(listener: UnixListener, host: SharedHost) {
        serve_listener_with(listener, host, Services::none())
    }

    pub fn serve_listener_with(listener: UnixListener, host: SharedHost, services: Services) {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let host = Arc::clone(&host);
                    let services = services.clone();
                    let spawned = std::thread::Builder::new()
                        .name("anterm-control-conn".into())
                        .spawn(move || {
                            if let Err(e) = serve_with(stream, host, services) {
                                log::warn!("control connection failed: {e}");
                            }
                        });
                    if let Err(e) = spawned {
                        log::warn!("could not start a control connection thread: {e}");
                    }
                }
                Err(e) => log::warn!("control accept failed: {e}"),
            }
        }
    }

    pub fn spawn_control_listener(host: SharedHost) -> io::Result<PathBuf> {
        spawn_control_listener_with(host, Services::none())
    }

    pub fn spawn_control_listener_with(
        host: SharedHost,
        services: Services,
    ) -> io::Result<PathBuf> {
        // Anchor uptime here, not at the first Status request. The GUI can host
        // the control listener too, and there `run_with` never runs — so
        // without this the OnceLock is first set by whoever asks, and every
        // answer reports a server that just started.
        super::server_started();
        let path = control_socket_path()?;
        let listener = bind_control_socket(&path)?;
        std::thread::Builder::new()
            .name("anterm-control-listener".into())
            .spawn(move || serve_listener_with(listener, host, services))?;
        Ok(path)
    }
}

#[cfg(all(unix, test))]
pub(crate) use sock::socket_path_in;
#[cfg(unix)]
pub use sock::{
    bind_control_socket, control_endpoint_answers, control_socket_path, serve_listener,
    serve_listener_with, spawn_control_listener, spawn_control_listener_with,
};

#[cfg(windows)]
mod wsock {
    use super::*;
    use crate::daemon::transport;
    use std::net::TcpListener;

    pub const CONTROL_PORT_FILE: &str = "control.port";

    pub fn control_endpoint_path() -> io::Result<PathBuf> {
        transport::port_path_named(CONTROL_PORT_FILE).ok_or_else(|| {
            io::Error::other("no config directory to record the control endpoint in")
        })
    }

    pub fn spawn_control_listener_with(
        host: SharedHost,
        services: Services,
    ) -> io::Result<PathBuf> {
        // See the unix arm: uptime is anchored where the listener opens, so a
        // GUI-hosted control server does not report itself as freshly started.
        super::server_started();
        let path = control_endpoint_path()?;
        // A control.port left by a dead daemon is stale: connecting to it
        // would pay the OS's refusal delay for nothing. The pidfile says
        // whether the daemon that wrote the port is still alive — when it is
        // gone the port cannot hold a live control server, and the bind below
        // simply overwrites the stale file.
        if !crate::daemon::spawn::recorded_daemon_is_dead() {
            if let Ok(live) = transport::connect_endpoint(CONTROL_PORT_FILE) {
                drop(live);
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!(
                        "a control server is already listening at {}",
                        transport::endpoint_display_named(CONTROL_PORT_FILE)
                    ),
                ));
            }
        }
        let (listener, token) =
            transport::bind_endpoint(CONTROL_PORT_FILE).map_err(io::Error::other)?;
        std::thread::Builder::new()
            .name("anterm-control-listener".into())
            .spawn(move || serve_listener_with(listener, token, host, services))?;
        Ok(path)
    }

    pub fn spawn_control_listener(host: SharedHost) -> io::Result<PathBuf> {
        spawn_control_listener_with(host, Services::none())
    }

    /// See the unix arm: a bind that failed only means "serve panes only" when
    /// something else is actually answering there.
    ///
    /// Gated on the pidfile the same way [`spawn_control_listener_with`] gates
    /// its own probe, and for the same reason: a TCP connect that completes
    /// proves only that *something* accepted on the recorded port, and a port a
    /// dead daemon wrote can have been recycled by a stranger since. When the
    /// daemon that wrote it is gone, nothing behind that port is ours.
    pub fn control_endpoint_answers() -> bool {
        !crate::daemon::spawn::recorded_daemon_is_dead()
            && transport::connect_endpoint(CONTROL_PORT_FILE).is_ok()
    }

    pub fn serve_listener_with(
        listener: TcpListener,
        token: transport::Token,
        host: SharedHost,
        services: Services,
    ) {
        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => {
                    transport::tune(&stream);
                    let host = Arc::clone(&host);
                    let services = services.clone();
                    let spawned = std::thread::Builder::new()
                        .name("anterm-control-conn".into())
                        .spawn(move || {
                            if let Err(e) = transport::check_endpoint_token(&mut stream, &token) {
                                log::warn!("control connection rejected: {e}");
                                return;
                            }
                            if let Err(e) = serve_with(stream, host, services) {
                                log::warn!("control connection failed: {e}");
                            }
                        });
                    if let Err(e) = spawned {
                        log::warn!("could not start a control connection thread: {e}");
                    }
                }
                Err(e) => log::warn!("control accept failed: {e}"),
            }
        }
    }

    pub fn connect_control() -> io::Result<std::net::TcpStream> {
        transport::connect_endpoint(CONTROL_PORT_FILE)
    }

    pub fn remove_control_endpoint() {
        transport::remove_endpoint(CONTROL_PORT_FILE);
    }
}

#[cfg(windows)]
pub use wsock::{
    CONTROL_PORT_FILE, connect_control, control_endpoint_answers, control_endpoint_path,
    remove_control_endpoint, spawn_control_listener, spawn_control_listener_with,
};
