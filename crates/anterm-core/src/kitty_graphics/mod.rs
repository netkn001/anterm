//! Streaming kitty-graphics-protocol extractor (APC `ESC _ G … ST`).
//!
//! The daemon-side counterpart to [`crate::osc`]: a tiny, resumable state
//! machine that pulls kitty graphics commands out of the raw PTY byte stream
//! *without* running a full VT parser. It exists for the same reason the OSC
//! sniffer does — the client's `alacritty_terminal` fork silently discards APC
//! sequences (`ESC _` routes to vte's `SosPmApcString`, whose payload bytes hit
//! a `_ => ()` arm), so an image transmitted this way never surfaces as a `Term`
//! event. We tap the bytes here instead.
//!
//! This lives daemon-side so the pixel payload can be lifted out of the stream
//! *before* it enters the replay ring — a full-window RGBA frame is hundreds of
//! KB, and letting it accumulate in the ring would make reattach replay
//! catastrophic. The daemon decodes here and forwards a compact out-of-band
//! frame to the client; the base64 text never rides the socket, and the VT
//! parser never has to chew through it.
//!
//! Scope: this handles the subset the wire actually carries in practice
//! (transmit-and-display, query, delete). Which transmission media we accept
//! depends on where the pane lives:
//!
//! - Direct (`t=d`, base64 inline) is always honored — it is the only medium
//!   that survives anterm's socket + SSH topology.
//! - File (`t=f`/`t=t`) and shared memory (`t=s`) are honored only on a *local*
//!   unix pane, where the name resolves on this host and the daemon can read it
//!   without anything crossing a tunnel. Everywhere else — a remote pane, or a
//!   non-unix host where [`MediumTransfer::resolve`] can't do the work — the
//!   probe is answered *unsupported* so a sender like `terminal-browser` falls
//!   back to `t=d` on its own rather than transmitting into a black hole.
//!
//! Protocol reference: <https://sw.kovidgoyal.net/kitty/graphics-protocol/>

#[cfg(test)]
mod tests;
mod tokenizer;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

pub use tokenizer::{ApcTokenizer, GraphicsSniffer, Segment, Sniffed};

/// Cap on the reassembled base64 of one chunked transmission.
///
/// This has to stay under [`crate::daemon::protocol::MAX_FRAME`] once decoded:
/// base64 shrinks by 3/4, and [`Image::encode_frame`] prepends [`HEADER_LEN`]
/// bytes, so the decoded payload must leave room for the header inside the wire
/// frame. Blowing past `MAX_FRAME` would make `write_frame` fail, which the
/// daemon's writer loop treats as fatal — one oversized image would drop the
/// client's whole connection instead of just that frame. 48 MiB of base64 is
/// ~36 MB of pixels, comfortably more than a 4K full-window RGBA frame (~33 MB)
/// and comfortably under the 64 MiB wire ceiling.
const MAX_TRANSMISSION_BASE64: usize = 48 << 20; // 48 MiB

/// Cap on the *resolved* pixel bytes of one image, whatever medium carried it.
/// Bounds the file/shm fast path (where the sender names an object whose size we
/// don't control) and backstops the direct path, keeping every frame we hand the
/// daemon inside [`crate::daemon::protocol::MAX_FRAME`].
pub const MAX_IMAGE_BYTES: usize = crate::daemon::protocol::MAX_FRAME - HEADER_LEN;

/// The kitty action key (`a=`). Only the variants anterm acts on are named; the
/// rest of the protocol (frames, animation, compose) folds into `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// `a=q` — query whether a transmission would succeed. We must reply.
    Query,
    /// `a=t` — transmit only (no display). Rare from our target sender.
    Transmit,
    /// `a=T` — transmit and display.
    TransmitAndDisplay,
    /// `a=p` — display a previously transmitted image.
    Display,
    /// `a=d` — delete image(s)/placement(s).
    Delete,
    /// Any action we don't specifically handle.
    Other,
}

/// The pixel format key (`f=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireFormat {
    /// `f=24` — 3 bytes/pixel RGB.
    Rgb,
    /// `f=32` — 4 bytes/pixel RGBA (the default, and what `terminal-browser` uses).
    Rgba,
    /// `f=100` — a PNG file; pixels stay encoded (decoded client-side).
    Png,
}

/// The transmission medium key (`t=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Medium {
    /// `t=d` — direct: the payload is (base64) the image data itself.
    Direct,
    /// `t=f` — a filesystem path to the data.
    File,
    /// `t=s` — a POSIX shared-memory object name.
    Shared,
    /// `t=t` — a temporary file (deleted after reading).
    TempFile,
}

/// Parsed kitty graphics control keys (the `k=v,k=v` list before the `;`).
///
/// Only the keys anterm needs are surfaced; unknown keys are ignored so forward
/// protocol additions degrade quietly rather than failing the whole command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Control {
    pub action: Action,
    pub format: WireFormat,
    pub medium: Medium,
    /// `o=z` — payload is zlib-compressed.
    pub compressed: bool,
    /// `i=` — client-assigned image id (0 = unset).
    pub id: u32,
    /// `I=` — client-assigned image number (0 = unset).
    pub number: u32,
    /// `p=` — placement id (0 = unset).
    pub placement: u32,
    /// `s=` — source pixel width.
    pub width: u32,
    /// `v=` — source pixel height.
    pub height: u32,
    /// `c=` — columns to display across (0 = derive from pixels).
    pub cols: u32,
    /// `r=` — rows to display down (0 = derive from pixels).
    pub rows: u32,
    /// `m=1` — more chunks follow.
    pub more: bool,
    /// `d=` — delete target (only meaningful for `a=d`).
    pub delete: u8,
    /// `q=` — suppress responses (1 = errors only, 2 = all).
    pub quiet: u8,
    /// `O=` — byte offset into a file/shm object (file/shm mediums only).
    pub offset: u32,
    /// `S=` — number of bytes to read from a file/shm object (0 = to end).
    pub size: u32,
}

impl Default for Control {
    fn default() -> Self {
        // Protocol defaults: a=t, f=32, t=d, no compression.
        Self {
            action: Action::Transmit,
            format: WireFormat::Rgba,
            medium: Medium::Direct,
            compressed: false,
            id: 0,
            number: 0,
            placement: 0,
            width: 0,
            height: 0,
            cols: 0,
            rows: 0,
            more: false,
            delete: 0,
            quiet: 0,
            offset: 0,
            size: 0,
        }
    }
}

impl Control {
    /// Parse the control section of a `_G` command — the `G…` payload up to (but
    /// not including) the first `;`. The leading `G` is part of the first key
    /// name's position, i.e. the payload is `Ga=T,f=32,…;<data>`; we accept the
    /// whole `G…;…` slice and split on the `;`.
    pub fn parse(payload: &[u8]) -> Option<Self> {
        // Strip the leading `G` identifier.
        let rest = payload.strip_prefix(b"G")?;
        let control = match rest.iter().position(|&b| b == b';') {
            Some(pos) => &rest[..pos],
            None => rest, // no payload section (e.g. `a=d`)
        };
        let mut c = Control::default();
        for pair in control.split(|&b| b == b',') {
            if pair.is_empty() {
                continue;
            }
            let mut kv = pair.splitn(2, |&b| b == b'=');
            let key = kv.next().unwrap_or(b"");
            let val = kv.next().unwrap_or(b"");
            let num = || parse_u32(val);
            match key {
                b"a" => {
                    c.action = match val {
                        b"q" => Action::Query,
                        b"t" => Action::Transmit,
                        b"T" => Action::TransmitAndDisplay,
                        b"p" => Action::Display,
                        b"d" => Action::Delete,
                        _ => Action::Other,
                    }
                }
                b"f" => {
                    c.format = match val {
                        b"24" => WireFormat::Rgb,
                        b"100" => WireFormat::Png,
                        _ => WireFormat::Rgba, // 32 and the default
                    }
                }
                b"t" => {
                    c.medium = match val {
                        b"f" => Medium::File,
                        b"s" => Medium::Shared,
                        b"t" => Medium::TempFile,
                        _ => Medium::Direct,
                    }
                }
                b"o" => c.compressed = val == b"z",
                b"i" => c.id = num(),
                b"I" => c.number = num(),
                b"p" => c.placement = num(),
                b"s" => c.width = num(),
                b"v" => c.height = num(),
                b"c" => c.cols = num(),
                b"r" => c.rows = num(),
                b"m" => c.more = val == b"1",
                b"d" => c.delete = val.first().copied().unwrap_or(0),
                // Clamp rather than truncate: `q=256` must not wrap to 0 and
                // turn a request for silence into a request for chatter.
                b"q" => c.quiet = num().min(u8::MAX as u32) as u8,
                b"O" => c.offset = num(),
                b"S" => c.size = num(),
                _ => {} // unknown key: ignore
            }
        }
        Some(c)
    }
}

fn parse_u32(bytes: &[u8]) -> u32 {
    let mut n: u32 = 0;
    for &b in bytes {
        if !b.is_ascii_digit() {
            return 0;
        }
        n = n.saturating_mul(10).saturating_add((b - b'0') as u32);
    }
    n
}

/// The `;`-separated data section of a `_G` command (may be empty).
fn payload_data(command: &[u8]) -> &[u8] {
    match command.iter().position(|&b| b == b';') {
        Some(pos) => &command[pos + 1..],
        None => &[],
    }
}

/// A reassembled image transmission, base64-decoded but *not yet* inflated.
///
/// The daemon does only the base64 decode (cheap, and it strips the 33% text
/// inflation) and forwards this over the socket; the client calls [`to_rgba8`]
/// to inflate and normalize to RGBA8. Keeping [`data`] compressed on the wire is
/// what makes a *remote* pane viable — inflating daemon-side would push tens of
/// MB per frame back through the SSH tunnel.
///
/// [`data`]: Image::data
/// [`to_rgba8`]: Image::to_rgba8
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub id: u32,
    pub number: u32,
    pub placement: u32,
    /// Source pixel dimensions.
    pub width: u32,
    pub height: u32,
    /// Requested display span in cells (0 = derive from pixels / cell size).
    pub cols: u32,
    pub rows: u32,
    /// The base64-decoded payload: zlib-compressed pixels when [`compressed`],
    /// otherwise raw pixels ([`WireFormat::Rgb`]/[`WireFormat::Rgba`]) or an
    /// encoded PNG file ([`WireFormat::Png`]).
    ///
    /// [`compressed`]: Image::compressed
    pub data: Vec<u8>,
    pub format: WireFormat,
    /// Whether [`data`](Image::data) is still zlib-compressed (`o=z`).
    pub compressed: bool,
}

impl Image {
    /// Inflate and normalize to tightly packed RGBA8 (`width*height*4`), the
    /// form the renderer wants. Runs client-side. Returns `None` for a PNG
    /// (whose decode needs the `image` crate the GUI owns, not this core) or on
    /// a malformed payload; PNG callers should decode [`data`](Image::data)
    /// themselves after checking [`format`](Image::format).
    ///
    /// The inflate is bounded by what `width`/`height` claim the pixels are:
    /// deflate expands ~1000:1 in the limit, so an unbounded `decompress_to_vec`
    /// here would let a payload well inside [`MAX_TRANSMISSION_BASE64`] balloon
    /// into tens of GB and OOM the client — and this runs in the GUI process, so
    /// it would take every pane down, not just the one that received the escape.
    /// The caller checks the inflated length against `width*height*4` anyway, so
    /// capping it up front costs nothing.
    pub fn to_rgba8(&self) -> Option<Vec<u8>> {
        if self.format == WireFormat::Png {
            return None;
        }
        let raw = if self.compressed {
            // Bound the inflate by the pixels the sender *claims* to be sending
            // (`width * height` at this format's bytes-per-pixel). Without a cap,
            // a high-ratio deflate payload well inside `MAX_TRANSMISSION_BASE64`
            // (which bounds only the *compressed* bytes) inflates to tens of GB
            // and OOMs the whole GUI process. A payload that decompresses past
            // its own declared dimensions is malformed, so we drop it.
            //
            // The declared size is itself attacker-chosen, so clamp it too:
            // `s=65535,v=65535` alone works out to 17 GB, which would hand the
            // bomb right back its allocation. No real frame comes near
            // `MAX_IMAGE_BYTES`, which is what the wire can carry anyway.
            let limit = self.decoded_len()?.min(MAX_IMAGE_BYTES);
            miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&self.data, limit).ok()?
        } else {
            self.data.clone()
        };
        Some(match self.format {
            WireFormat::Rgb => rgb_to_rgba(&raw),
            _ => raw, // Rgba
        })
    }

    /// Like [`to_rgba8`](Image::to_rgba8) but *moves* the pixel buffer out of the
    /// image on the uncompressed fast path instead of cloning it, leaving
    /// [`data`](Image::data) empty. This is the client hot path: a full-window
    /// browser frame is ~26 MiB of RGBA, and the uncompressed shm/file transport
    /// hands it to us already in the final `f=32` layout — so the clone
    /// [`to_rgba8`] does there is pure waste at video frame rates. Only the pixel
    /// buffer moves; the caller's `cols`/`rows`/`id`/`placement` metadata is
    /// untouched. The compressed inflate and the PNG guard are identical to
    /// [`to_rgba8`]; `f=24` still repacks (RGB→RGBA changes the length).
    ///
    /// [`to_rgba8`]: Image::to_rgba8
    pub fn take_rgba8(&mut self) -> Option<Vec<u8>> {
        if self.format == WireFormat::Png {
            return None;
        }
        let data = std::mem::take(&mut self.data);
        if self.compressed {
            // Same bounded inflate as `to_rgba8` — see that method for why the
            // limit is clamped to `MAX_IMAGE_BYTES` as well as the declared size.
            let limit = self.decoded_len()?.min(MAX_IMAGE_BYTES);
            let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&data, limit).ok()?;
            return Some(match self.format {
                WireFormat::Rgb => rgb_to_rgba(&raw),
                _ => raw,
            });
        }
        Some(match self.format {
            WireFormat::Rgb => rgb_to_rgba(&data),
            _ => data, // Rgba — moved out of the frame, not cloned.
        })
    }

    /// The byte length of this image's *decoded* (pre-`rgb_to_rgba`) pixels —
    /// `width * height * bytes_per_pixel` for the wire format. `None` on overflow
    /// or a zero-sized/PNG image, which have no fixed raw length.
    fn decoded_len(&self) -> Option<usize> {
        let bpp = match self.format {
            WireFormat::Rgb => 3usize,
            WireFormat::Rgba => 4usize,
            WireFormat::Png => return None,
        };
        (self.width as usize)
            .checked_mul(self.height as usize)?
            .checked_mul(bpp)
            .filter(|&n| n != 0)
    }

    /// Encode for the daemon→client [`crate::daemon::protocol::DaemonMsg::Image`]
    /// frame: a fixed 30-byte header carrying the metadata, then the raw `data`
    /// bytes appended verbatim. A JSON envelope would base64-inflate the pixel
    /// payload ~1.33×; this keeps it byte-for-byte, which matters at video frame
    /// rates. See [`decode_frame`](Image::decode_frame).
    pub fn encode_frame(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.data.len());
        out.extend_from_slice(&self.id.to_le_bytes());
        out.extend_from_slice(&self.number.to_le_bytes());
        out.extend_from_slice(&self.placement.to_le_bytes());
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&self.cols.to_le_bytes());
        out.extend_from_slice(&self.rows.to_le_bytes());
        out.push(match self.format {
            WireFormat::Rgb => 24,
            WireFormat::Rgba => 32,
            WireFormat::Png => 100,
        });
        out.push(u8::from(self.compressed));
        out.extend_from_slice(&self.data);
        out
    }

    /// Reconstruct from an [`encode_frame`](Image::encode_frame) payload.
    pub fn decode_frame(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < HEADER_LEN {
            return None;
        }
        let u32_at = |o: usize| {
            u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
        };
        let format = match bytes[28] {
            24 => WireFormat::Rgb,
            100 => WireFormat::Png,
            _ => WireFormat::Rgba,
        };
        Some(Image {
            id: u32_at(0),
            number: u32_at(4),
            placement: u32_at(8),
            width: u32_at(12),
            height: u32_at(16),
            cols: u32_at(20),
            rows: u32_at(24),
            format,
            compressed: bytes[29] != 0,
            data: bytes[HEADER_LEN..].to_vec(),
        })
    }

    /// Like [`decode_frame`](Image::decode_frame) but consumes the frame buffer,
    /// reusing its allocation for the pixel payload instead of copying the tail
    /// into a fresh `Vec`. The client owns the frame `Vec` straight off the
    /// socket, and the pixels are the overwhelming majority of it (a ~26 MiB
    /// RGBA frame behind a 30-byte header), so draining the header in place turns
    /// a full-frame copy into a cheap front shift. Behaviourally identical to
    /// `decode_frame` — same header parse, same fields.
    pub fn decode_frame_owned(mut frame: Vec<u8>) -> Option<Self> {
        if frame.len() < HEADER_LEN {
            return None;
        }
        let u32_at = |o: usize| {
            u32::from_le_bytes([frame[o], frame[o + 1], frame[o + 2], frame[o + 3]])
        };
        let id = u32_at(0);
        let number = u32_at(4);
        let placement = u32_at(8);
        let width = u32_at(12);
        let height = u32_at(16);
        let cols = u32_at(20);
        let rows = u32_at(24);
        let format = match frame[28] {
            24 => WireFormat::Rgb,
            100 => WireFormat::Png,
            _ => WireFormat::Rgba,
        };
        let compressed = frame[29] != 0;
        // Reuse the socket buffer as the pixel buffer: drop the header off the
        // front rather than allocating a new `Vec` for `frame[HEADER_LEN..]`.
        frame.drain(..HEADER_LEN);
        Some(Image {
            id,
            number,
            placement,
            width,
            height,
            cols,
            rows,
            format,
            compressed,
            data: frame,
        })
    }
}

/// Byte length of the [`Image::encode_frame`] header: seven `u32` fields, then
/// the format and compression bytes.
const HEADER_LEN: usize = 7 * 4 + 1 + 1;

impl MediumTransfer {
    /// Resolve the referenced bytes into an [`Image`] by reading the file or
    /// `mmap`ing the POSIX shm object this transfer names, then unlinking it (a
    /// [`Medium::TempFile`] and any shm object are one-shot handoffs the sender
    /// expects the terminal to consume and remove; a plain [`Medium::File`] is
    /// left in place). Runs on the daemon reader thread of a *local* pane, where
    /// the name resolves on this host. Returns `None` if the name is unusable or
    /// the object can't be read.
    ///
    /// The pixels come back *uncompressed* unless the sender set `o=z`: a sender
    /// that reaches for shared memory does so precisely to avoid the zlib the
    /// inline path forces, so this is the fast path that skips the client-side
    /// inflate entirely.
    #[cfg(unix)]
    pub fn resolve(&self) -> Option<Image> {
        let data = match self.medium {
            Medium::Shared => self.read_shared()?,
            Medium::File | Medium::TempFile => self.read_file()?,
            // Direct never becomes a MediumTransfer.
            Medium::Direct => return None,
        };
        Some(Image {
            id: self.id,
            number: self.number,
            placement: self.placement,
            width: self.width,
            height: self.height,
            cols: self.cols,
            rows: self.rows,
            data,
            format: self.format,
            compressed: self.compressed,
        })
    }

    #[cfg(not(unix))]
    pub fn resolve(&self) -> Option<Image> {
        None
    }

    /// The `offset..offset+size` slice of `buf` (or `offset..` when `size == 0`),
    /// copied out. `None` if `offset` itself falls outside the object; an `S=`
    /// that runs past the end is *truncated* to what's there rather than
    /// discarding the frame, which is what kitty does.
    #[cfg(unix)]
    fn slice_region(&self, buf: &[u8]) -> Option<Vec<u8>> {
        let start = self.offset as usize;
        if start > buf.len() {
            return None;
        }
        let end = if self.size == 0 {
            buf.len()
        } else {
            start.saturating_add(self.size as usize).min(buf.len())
        };
        buf.get(start..end).map(<[u8]>::to_vec)
    }

    #[cfg(unix)]
    fn read_file(&self) -> Option<Vec<u8>> {
        use std::io::Read as _;
        use std::os::unix::ffi::OsStringExt;
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(self.name.clone()));

        // Read through a bounded reader, not `fs::read`. The name is attacker
        // -reachable (any program that can write to the pty picks it), and
        // `fs::read` on `/dev/zero` never returns — on the daemon *reader*
        // thread, which would wedge the pane's whole output path. Refusing
        // anything that isn't a regular file also keeps us off fifos and
        // devices, where the open itself can block.
        let file = std::fs::File::open(&path).ok()?;
        let meta = file.metadata().ok()?;
        if !meta.is_file() || meta.len() as usize > MAX_IMAGE_BYTES {
            return None;
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        file.take(MAX_IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return None;
        }

        let out = self.slice_region(&bytes)?;
        // A temp file is the sender's one-shot handoff: remove it after reading
        // so the browser's per-frame temp files don't pile up. A named `t=f`
        // file is the sender's to manage; leave it.
        //
        // Only unlink inside a known temp directory. `name` is an arbitrary path
        // out of an escape sequence — `cat`ing a hostile file is enough to send
        // one — so an unqualified `remove_file` here would delete anything the
        // user can, `~/.ssh/id_ed25519` included. The spec requires this check
        // for exactly that reason.
        if self.medium == Medium::TempFile && path_is_in_temp_dir(&path) {
            let _ = std::fs::remove_file(&path);
        }
        Some(out)
    }

    /// `shm_open` + `mmap` the object, copy out the requested region, then
    /// `shm_unlink` it (the sender allocates a fresh object per frame and
    /// expects the terminal to reclaim it — matching kitty/ghostty).
    #[cfg(unix)]
    fn read_shared(&self) -> Option<Vec<u8>> {
        use std::os::raw::c_void;
        // A POSIX shm name is a single `/`-prefixed component — no embedded
        // separators, no `..`. Some platforms resolve `shm_open` against the
        // filesystem, where a name like `/../../etc/passwd` would escape the shm
        // namespace and reach a real path we'd then `shm_unlink`.
        if !shm_name_is_wellformed(&self.name) {
            return None;
        }
        // The name must be a C string; kitty shm names look like `/px-…`.
        let cname = std::ffi::CString::new(self.name.clone()).ok()?;
        // SAFETY: FFI. `shm_open` with O_RDONLY on a name the sender created;
        // we only ever read, mmap read-only, and always munmap/close/unlink on
        // every exit path below.
        unsafe {
            let fd = libc::shm_open(cname.as_ptr(), libc::O_RDONLY, 0);
            if fd < 0 {
                return None;
            }
            // Size the mapping from the object itself; the sender may not send
            // `S=`, and mapping past the end would fault on access.
            // Refuse an object bigger than a frame can carry rather than mapping
            // and copying it out only for the send to fail downstream.
            let mut st: libc::stat = std::mem::zeroed();
            if libc::fstat(fd, &mut st) != 0
                || st.st_size <= 0
                || st.st_size as u64 > MAX_IMAGE_BYTES as u64
            {
                libc::close(fd);
                libc::shm_unlink(cname.as_ptr());
                return None;
            }
            let len = st.st_size as usize;
            let addr = libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            );
            libc::close(fd);
            if addr == libc::MAP_FAILED {
                libc::shm_unlink(cname.as_ptr());
                return None;
            }
            let mapped = std::slice::from_raw_parts(addr as *const u8, len);
            let out = self.slice_region(mapped);
            libc::munmap(addr as *mut c_void, len);
            // One-shot: the sender expects us to reclaim the object.
            libc::shm_unlink(cname.as_ptr());
            out
        }
    }
}

/// Whether `path` sits inside a directory the platform hands out for temp files,
/// which is the only place a `t=t` handoff may be unlinked.
///
/// Compares *canonicalized* paths so `/tmp/../home/me/.ssh/id_ed25519` — which
/// has the right prefix textually — doesn't pass. On macOS `TMPDIR` is a
/// per-user path under `/var/folders/…` that canonicalizes through the
/// `/private` symlink, so `std::env::temp_dir()` is canonicalized too rather
/// than compared raw.
#[cfg(unix)]
fn path_is_in_temp_dir(path: &std::path::Path) -> bool {
    let Ok(real) = path.canonicalize() else {
        return false;
    };
    // `/dev/shm` is where a `t=t` sender that wanted shm-like semantics without
    // `shm_open` puts its handoff; kitty accepts it alongside the temp dirs.
    let candidates = [
        std::env::temp_dir(),
        std::path::PathBuf::from("/tmp"),
        std::path::PathBuf::from("/dev/shm"),
    ];
    candidates.iter().any(|dir| {
        dir.canonicalize()
            .is_ok_and(|d| real.starts_with(&d) && real != d)
    })
}

/// Whether `name` is a well-formed POSIX shm object name: a leading `/` followed
/// by one non-empty component with no further separators and no `.`/`..`.
#[cfg(unix)]
fn shm_name_is_wellformed(name: &[u8]) -> bool {
    // POSIX caps the name at NAME_MAX; 255 is the floor every platform we build
    // for meets, and no real sender comes close.
    if name.len() < 2 || name.len() > 255 || name[0] != b'/' {
        return false;
    }
    let rest = &name[1..];
    !rest.contains(&b'/') && !rest.contains(&0) && rest != b"." && rest != b".."
}

/// A delete request distilled from an `a=d` command, in the compact form the
/// daemon forwards to the client's image store. The full kitty delete grammar is
/// rich (by id, by placement, by cell, by z-index, …); the client only needs the
/// target selector plus the id/placement it may scope to, which is all any sender
/// anterm targets — most often `d=A` (delete everything) — actually uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageDelete {
    /// The `d=` selector byte (e.g. `A`/`a` = all, `i` = by id, `p` = by
    /// placement). Uppercase variants also free the image data in kitty; the
    /// client frees unconditionally, so case only affects which images match.
    pub target: u8,
    pub id: u32,
    pub placement: u32,
}

impl ImageDelete {
    pub fn from_control(c: &Control) -> Self {
        Self {
            // A bare `a=d` with no `d=` means "delete all visible placements",
            // which kitty spells `a` — normalize the unset case to it.
            target: if c.delete == 0 { b'a' } else { c.delete },
            id: c.id,
            placement: c.placement,
        }
    }

    /// A fixed 9-byte frame: the selector byte then id and placement (LE).
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(9);
        out.push(self.target);
        out.extend_from_slice(&self.id.to_le_bytes());
        out.extend_from_slice(&self.placement.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 9 {
            return None;
        }
        Some(Self {
            target: bytes[0],
            id: u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]),
            placement: u32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]),
        })
    }
}

/// What a fed command turned into. Query/Delete carry no pixels; the caller
/// writes the query reply back to the PTY and applies deletes to its store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// An `a=q` probe. `reply` is the bytes to write to the PTY; `honored` says
    /// whether we accepted the requested medium (for logging/metrics only).
    Query { reply: Vec<u8>, honored: bool },
    /// A complete image transmission (`a=T`/`a=t`).
    Image(Image),
    /// A transmission whose pixels live in a file or POSIX shm object rather
    /// than inline in the escape. The parser can't read the filesystem (it must
    /// stay pure and host-agnostic), so it hands the reference to the daemon
    /// pane — which is co-located with the sender on a *local* pane — to `mmap`
    /// / read and unlink. This is the zero-copy, zero-inflate path a sender like
    /// `terminal-browser` prefers; see [`MediumTransfer`].
    ImageFromMedium(MediumTransfer),
    /// An `a=d` delete request.
    Delete(Control),
}

/// A file/shm transmission the daemon must resolve into pixels. The `name` is
/// the (base64-decoded) payload the sender put after the `;`: a filesystem path
/// for [`Medium::File`]/[`Medium::TempFile`], or a POSIX shm object name for
/// [`Medium::Shared`]. `offset`/`size` bound the region to read (`size == 0`
/// means "to end of object"). All the metadata needed to build the final
/// [`Image`] rides along so the daemon does no parsing of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediumTransfer {
    pub medium: Medium,
    pub name: Vec<u8>,
    pub offset: u32,
    pub size: u32,
    pub id: u32,
    pub number: u32,
    pub placement: u32,
    pub width: u32,
    pub height: u32,
    pub cols: u32,
    pub rows: u32,
    pub format: WireFormat,
    /// Whether the referenced bytes are zlib-compressed (`o=z`). A sender that
    /// picks shm/file for speed sends raw pixels, but honor the flag regardless.
    pub compressed: bool,
}

/// State for reassembling a chunked (`m=1`) direct transmission. Kitty
/// serializes chunked transmissions — only one is in flight at a time — so a
/// single pending accumulator suffices.
#[derive(Default)]
struct Pending {
    control: Control,
    /// Accumulated *base64* text across chunks (decoded once, at the end).
    base64: Vec<u8>,
}

/// Reassembles and decodes kitty graphics commands emitted by [`ApcTokenizer`].
///
/// Feed it each `_G` command payload; it returns zero or more [`Event`]s. It
/// owns the chunk-reassembly buffer and the base64/zlib decode, so the daemon
/// pane just forwards the resulting [`Image`] out-of-band and writes any
/// [`Event::Query`] reply to the PTY.
#[derive(Default)]
pub struct GraphicsParser {
    pending: Option<Pending>,
    /// Whether the sender shares this host's filesystem (a local pane). Only
    /// then can we honor file/shm transfer, whose names are host-local; a pane
    /// running over SSH must stay on inline `t=d` so the pixels ride the tunnel.
    local: bool,
}

impl GraphicsParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// A parser that may honor file/shm transfer because the sender is on this
    /// host (see [`GraphicsParser::local`]).
    pub fn new_local(local: bool) -> Self {
        Self {
            local,
            ..Self::default()
        }
    }

    /// Whether a `t=f`/`t=t`/`t=s` transfer can actually be resolved here: the
    /// sender has to share this host's filesystem (a local pane), and
    /// [`MediumTransfer::resolve`] has to have a real implementation on this
    /// platform (it is unix-only). Both `query_reply` and `finalize` route
    /// through this so what we advertise and what we accept can't drift apart.
    fn honors_indirect_media(&self) -> bool {
        self.local && cfg!(unix)
    }

    /// Feed one complete `_G` command payload (the bytes [`ApcTokenizer`]
    /// delivers). Returns an [`Event`] when a command completes (a query, a
    /// finished image, or a delete); returns `None` for an intermediate chunk of
    /// a still-incomplete transmission.
    pub fn feed(&mut self, command: &[u8]) -> Option<Event> {
        let control = Control::parse(command)?;
        let data = payload_data(command);

        // A continuation chunk of a chunked transmission carries only `m=…` — no
        // `a=` — so its action defaults to `Transmit`. Route anything while a
        // transmission is pending straight to the chunk accumulator, which keeps
        // the *first* chunk's real control keys.
        if self.pending.is_some() {
            return self.accept_chunk(control, data);
        }

        match control.action {
            Action::Query => self.query_reply(&control),
            Action::Delete => Some(Event::Delete(control)),
            Action::TransmitAndDisplay | Action::Transmit => self.accept_chunk(control, data),
            // `a=p` places an image transmitted by an *earlier* command, which
            // needs the stored-image table this parser deliberately doesn't
            // keep. Routing it through `accept_chunk` would emit an `Image` with
            // an empty payload — a frame the client can only throw away — so
            // drop it here instead. Senders that split transmit from placement
            // get nothing; `a=T` (the shape every sender anterm targets uses)
            // is unaffected.
            Action::Display => None,
            // An action we don't handle, with nothing in flight: drop it.
            Action::Other => None,
        }
    }

    /// Append a (possibly first) chunk; finalize when `m` is not set.
    fn accept_chunk(&mut self, control: Control, data: &[u8]) -> Option<Event> {
        // The first chunk carries the real control keys; later chunks repeat
        // only `m=`. Preserve the first chunk's control across the transmission.
        let is_first = self.pending.is_none();
        if is_first {
            self.pending = Some(Pending {
                control,
                base64: Vec::new(),
            });
        }
        let pending = self.pending.as_mut()?;
        pending.base64.extend_from_slice(data);
        if pending.base64.len() > MAX_TRANSMISSION_BASE64 {
            log::debug!(
                "kitty graphics: abandoning a transmission past {MAX_TRANSMISSION_BASE64} base64 bytes"
            );
            self.pending = None; // abandon an oversized transmission
            return None;
        }
        if control.more {
            return None; // more chunks to come
        }
        // Complete: decode and emit.
        let Pending { control, base64 } = self.pending.take()?;
        self.finalize(control, &base64)
    }

    fn finalize(&self, control: Control, base64: &[u8]) -> Option<Event> {
        // Base64-decode the payload once. For direct transmission this *is* the
        // (still-compressed) pixels; for file/shm it's the path/object name.
        let data = BASE64.decode(base64).ok()?;

        // File/shm transfer: the payload names bytes on this host. Hand the
        // reference to the daemon to resolve — but only on a local pane, where
        // the name is meaningful and reading it can't leak across an SSH tunnel.
        // A `query_reply` earlier already refused these mediums on a remote
        // pane, so a well-behaved sender never reaches here; the guard is
        // belt-and-suspenders.
        if control.medium != Medium::Direct {
            if !self.honors_indirect_media() {
                return None;
            }
            return Some(Event::ImageFromMedium(MediumTransfer {
                medium: control.medium,
                name: data,
                offset: control.offset,
                size: control.size,
                id: control.id,
                number: control.number,
                placement: control.placement,
                width: control.width,
                height: control.height,
                cols: control.cols,
                rows: control.rows,
                format: control.format,
                compressed: control.compressed,
            }));
        }

        // Direct: inflation is deferred to the client (`to_rgba8`) so the
        // payload rides the socket — and any SSH tunnel — compressed.
        Some(Event::Image(Image {
            id: control.id,
            number: control.number,
            placement: control.placement,
            width: control.width,
            height: control.height,
            cols: control.cols,
            rows: control.rows,
            data,
            format: control.format,
            compressed: control.compressed,
        }))
    }

    /// Build the `a=q` reply. On a local unix pane we honor direct, file, and
    /// shm transfer, so any of those probes gets `OK`; otherwise only direct is
    /// honored and a `t=f`/`t=s`/`t=t` probe gets an error — which makes a
    /// sender like `terminal-browser` fall back to inline `t=d` on its own.
    ///
    /// The answer has to track what [`MediumTransfer::resolve`] can actually do,
    /// including on the platform axis: it is unix-only, so replying `OK` to a
    /// file/shm probe on Windows would talk a sender into a medium whose every
    /// frame we then silently discard, leaving the pane blank.
    fn query_reply(&self, control: &Control) -> Option<Event> {
        let honored = control.medium == Medium::Direct || self.honors_indirect_media();
        // `q=` asks us to stay quiet: 1 suppresses success replies, 2 suppresses
        // failures too. This matters because the reply is written back to the
        // *PTY* — it arrives as if the user had typed it. A sender that asked
        // for silence and gets `\x1b_Gi=1;OK\x1b\` anyway has those bytes land
        // in its stdin, or, if it already exited, on the shell's input line.
        if control.quiet >= 2 || (control.quiet == 1 && honored) {
            return None;
        }
        let status: &[u8] = if honored { b"OK" } else { b"ENOTSUPPORTED" };
        // Echo back the id (preferred) or number the sender used, exactly as
        // kitty does, so the sender can correlate the reply.
        let mut reply = Vec::with_capacity(32);
        reply.extend_from_slice(b"\x1b_G");
        if control.id != 0 {
            reply.extend_from_slice(format!("i={}", control.id).as_bytes());
        } else if control.number != 0 {
            reply.extend_from_slice(format!("I={}", control.number).as_bytes());
        } else {
            reply.extend_from_slice(b"i=0");
        }
        reply.push(b';');
        reply.extend_from_slice(status);
        reply.extend_from_slice(b"\x1b\\");
        Some(Event::Query { reply, honored })
    }
}

/// Expand tightly packed RGB into RGBA with an opaque alpha channel.
fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for px in rgb.chunks_exact(3) {
        out.extend_from_slice(px);
        out.push(0xff);
    }
    out
}
