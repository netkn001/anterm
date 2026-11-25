use std::path::{Path, PathBuf};

/// How many readings of one token the candidate ladder may produce. Each one
/// costs a probe per root, so the cap is what keeps a separator-dense token
/// from turning a hover into a burst of filesystem calls.
pub(super) const MAX_FILE_CANDIDATES: usize = 8;

/// The longest quoted span read as one path with spaces in it. Windows'
/// classic `MAX_PATH`: past this a quote is holding a sentence, not a path.
pub(super) const MAX_QUOTED_PATH: usize = 260;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LinkTarget {
    Url(String),
    File {
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
        /// Set when the path is a directory, which is a link to a *place* —
        /// the file tree shows it, the editor cannot.
        is_dir: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LinkMatch {
    pub start: usize,
    pub end: usize,
    pub target: LinkTarget,
}

#[cfg(test)]
pub(crate) fn url_at(text: &str, col: usize) -> Option<String> {
    url_span_at(text, col).map(|(_, _, url)| url)
}

/// What checking one candidate path came back with.
///
/// The third arm is what makes remote panes work at all. A local pane answers
/// out of the filesystem in microseconds, but a pane whose paths live on
/// another machine has to ask that machine, and the answer arrives frames
/// later — long after the mouse event that wanted it. [`Probe::Unknown`] is
/// that gap: it means "nobody has asked yet", and the prober is expected to go
/// ask so the *next* look finds a real answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Probe {
    /// Something is there. Whether it is a directory travels with the answer:
    /// a directory link belongs in the file tree, not in the editor.
    Hit {
        is_dir: bool,
    },
    Miss,
    Unknown,
}

/// Answers for paths on the machine anterm itself is running on.
pub(crate) fn local_probe(path: &Path, require_file: bool) -> Probe {
    if path.is_file() {
        return Probe::Hit { is_dir: false };
    }
    match !require_file && path.is_dir() {
        true => Probe::Hit { is_dir: true },
        false => Probe::Miss,
    }
}

/// Which language a pane's paths are written in.
///
/// The machine anterm runs on and the machine a pane's paths live on need not
/// agree, and `std::path` only ever speaks the first one's dialect. On a
/// Windows client that is the whole difference between a link and nothing: a
/// leading `/` is not absolute to `Path` there, so `/etc/hosts` printed by a
/// Linux pane used to be measured from that pane's directory instead of
/// standing alone, and a relative `src/lib.rs` was joined onto it with a
/// backslash the far side has never heard of.
///
/// The two arms are the two dialects, not the two operating systems: a WSL
/// distro and an SSH host both speak [`PathStyle::Posix`] whatever the client
/// is, and a pane on this machine speaks [`PathStyle::NATIVE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathStyle {
    /// `/`-rooted and `/`-joined. Everything a Unix host, a WSL distro or a
    /// Git-Bash-style shell prints.
    Posix,
    /// Rooted by a drive letter or a UNC share, joined with `\`.
    Windows,
}

impl Default for PathStyle {
    fn default() -> Self {
        Self::NATIVE
    }
}

impl PathStyle {
    /// The dialect the machine anterm is running on speaks.
    pub const NATIVE: PathStyle = match cfg!(windows) {
        true => PathStyle::Windows,
        false => PathStyle::Posix,
    };

    /// The dialect a host that called `sample` one of its own directories
    /// speaks: a drive letter or a UNC share is a Windows host's, and anything
    /// else is read as POSIX.
    ///
    /// Only a directory that names a *Windows* root counts as one, rather than
    /// every directory that does not name a POSIX one. A cwd that is neither —
    /// a relative one, or an empty one from a host that has not settled yet —
    /// is the same "nothing to go on" as no cwd at all, and has to fall the
    /// same way; reading it as Windows would spell a Linux host's paths with
    /// backslashes on the strength of a directory it never really reported.
    ///
    /// This is inference from what a host says about itself in passing. It is
    /// sound in the direction that matters: nothing but a POSIX host reports a
    /// `/`-rooted cwd.
    pub fn of_dir(sample: &Path) -> Self {
        match PathStyle::Windows.is_absolute(&sample.to_string_lossy()) {
            true => PathStyle::Windows,
            false => PathStyle::Posix,
        }
    }

    /// Whether a token says for itself which filesystem root it hangs off.
    ///
    /// Deliberately textual rather than [`Path::is_absolute`], which answers
    /// for *this* machine: the same string has to be read the pane's way on
    /// every client, or a link works on a Mac and not on the Windows box next
    /// to it.
    pub fn is_absolute(self, path: &str) -> bool {
        match self {
            PathStyle::Posix => path.starts_with('/'),
            // A UNC share, or a drive letter with a separator behind it.
            // `C:foo` is drive-*relative* and deliberately not included, which
            // is what `Path::is_absolute` says on Windows too.
            PathStyle::Windows => path.starts_with("\\\\") || drive_root(path).is_some(),
        }
    }

    /// The one path a token that names its own root stands for, spelled the
    /// way the pane's host can look it up — `None` for a token that has to be
    /// measured from somewhere.
    ///
    /// A Windows pane gets its path back with every separator a `\` and the
    /// drive letter upper-cased. `c:/Users/me/a.png` is what Node prints on
    /// Windows, and while the filesystem takes either slash, Explorer does
    /// not: handed a forward-slashed path to open or `/select,` it shows
    /// Documents and reports success.
    ///
    /// A POSIX pane has no drives, but it may well be a WSL distro looking at
    /// the Windows machine it runs on — a Windows tool run through interop, a
    /// `wslpath -w`, a build log from the other side. There `C:\Users` is
    /// `/mnt/c/Users`, and that is what the host is asked about. Anywhere else
    /// the mount is not there and the probe simply misses, so nothing
    /// underlines; that beats measuring `C:\Users` from the pane's directory,
    /// which could only ever be nonsense.
    pub fn rooted(self, path: &str) -> Option<String> {
        match self {
            PathStyle::Posix if path.starts_with('/') => Some(path.to_string()),
            PathStyle::Posix => {
                let (drive, rest) = drive_root(path)?;
                let rest = rest.replace('\\', "/");
                Some(format!(
                    "/mnt/{}/{}",
                    drive.to_ascii_lowercase(),
                    rest.trim_start_matches('/')
                ))
            }
            PathStyle::Windows => self.is_absolute(path).then(|| windows_spelling(path)),
        }
    }

    /// Whether a token only means something against the current directory of
    /// one particular drive: `C:notes.txt`, or `C:` on its own.
    ///
    /// There is no such thing as "the current directory of drive C" for a
    /// pane — only for a process — so the one answer that would exist is anterm's
    /// own, which has nothing to do with what the pane printed. And a bare
    /// letter and a colon is far more often prose than a path: `a:b`, `x:y`.
    fn is_drive_relative(self, path: &str) -> bool {
        let mut chars = path.chars();
        self == PathStyle::Windows
            && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
            && chars.next() == Some(':')
            && !matches!(chars.next(), Some('\\' | '/'))
    }

    /// `rel` measured from `root`, spelled the way the pane's host spells it.
    ///
    /// Both arms join textually because `Path::join` would reach for this
    /// machine's separator: on Windows it turns `/home/u` and `src/lib.rs`
    /// into `/home/u\src/lib.rs`, which the Linux box on the other end of the
    /// probe cannot stat, and on a Mac it turns `C:\proj` and `src\lib.rs`
    /// into `C:\proj/src\lib.rs` for a Windows host.
    ///
    /// A Windows `rel` that starts with a separator is rooted on the root's
    /// own drive (or share), the way `cd \etc` is in a `cmd.exe` sitting on C:.
    pub fn join(self, root: &Path, rel: &str) -> PathBuf {
        let root = root.to_string_lossy();
        match self {
            PathStyle::Posix => PathBuf::from(format!("{}/{rel}", root.trim_end_matches('/'))),
            PathStyle::Windows => {
                let joined = match rel.starts_with(['\\', '/']) {
                    true => format!("{}{rel}", windows_prefix(&root)),
                    false => format!("{}\\{rel}", root.trim_end_matches(['\\', '/'])),
                };
                PathBuf::from(windows_spelling(&joined))
            }
        }
    }
}

/// The drive letter a path starts with, and everything after the colon — only
/// when a separator follows it, which is what makes it absolute. `C:\x` and
/// `c:/x` are; `C:x`, `C:` and `12:30` are not.
fn drive_root(path: &str) -> Option<(char, &str)> {
    let mut chars = path.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic)?;
    let rest = path.strip_prefix(drive)?.strip_prefix(':')?;
    rest.starts_with(['\\', '/']).then_some((drive, rest))
}

/// The part of a Windows path that names its drive or share: `C:` out of
/// `C:\proj`, `\\server\share` out of `\\server\share\proj`. Empty when there
/// is none to take.
pub(super) fn windows_prefix(path: &str) -> &str {
    if drive_root(path).is_some() || path.len() == 2 && path.ends_with(':') {
        return &path[..2];
    }
    if path.starts_with("\\\\") || path.starts_with("//") {
        // Past the two leading separators, the server, and the share.
        let mut seps = path
            .char_indices()
            .skip(2)
            .filter(|&(_, c)| matches!(c, '\\' | '/'));
        seps.next();
        return match seps.next() {
            Some((end, _)) => &path[..end],
            None => path,
        };
    }
    ""
}

/// `path` spelled the way `style`'s own tools want it — for handing a path to
/// something outside anterm, like the OS opener. Only a Windows spelling
/// changes, and only a path that is valid UTF-8: rewriting anything else
/// through a lossy string would name a different file.
#[cfg(test)]
pub(crate) fn spelled_for(style: PathStyle, path: &Path) -> PathBuf {
    match (style, path.to_str()) {
        (PathStyle::Windows, Some(text)) => PathBuf::from(windows_spelling(text)),
        _ => path.to_path_buf(),
    }
}

/// A Windows path the way Windows itself writes one: `\` throughout and an
/// upper-case drive letter. Every Windows API takes `/` too, but Explorer —
/// the system opener and the "Reveal" menu item — does not.
fn windows_spelling(path: &str) -> String {
    let mut out = path.replace('/', "\\");
    if let Some((drive, _)) = drive_root(&out)
        && drive.is_ascii_lowercase()
    {
        out.replace_range(..1, &drive.to_ascii_uppercase().to_string());
    }
    out
}

/// Where a relative path printed by a pane is measured from.
///
/// `local_home` is the part that is easy to miss. `~` has to become a real
/// directory before anything can be looked up, and the only clue a pane
/// usually offers is its own cwd; when that does not reveal a home, this
/// machine's `$HOME` is the last resort. Sound for a pane on this machine, and
/// a fabrication for one whose paths live elsewhere — `/Users/me/.zshrc` is
/// not what `~/.zshrc` means on a Linux box, and asking that box about it is
/// at best a miss and at worst somebody else's file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LinkRoots {
    /// Directories a relative path is tried against, nearest first.
    pub dirs: Vec<PathBuf>,
    /// Whether this machine's `$HOME` may stand in for a `~` the roots cannot
    /// explain.
    pub local_home: bool,
    /// How the pane's host spells the paths it prints.
    pub style: PathStyle,
}

impl LinkRoots {
    /// Roots on the machine anterm is running on, where `$HOME` means what it
    /// says and paths are spelled this OS's way.
    #[cfg(test)]
    pub fn local(dirs: Vec<PathBuf>) -> Self {
        Self {
            dirs,
            local_home: true,
            style: PathStyle::NATIVE,
        }
    }

    /// The directory the pane is in — the first root, and the one a `~` is
    /// read out of.
    pub fn cwd(&self) -> Option<&Path> {
        self.dirs.first().map(PathBuf::as_path)
    }
}

pub(crate) fn link_at(
    text: &str,
    col: usize,
    roots: &LinkRoots,
    include_files: bool,
    probe: &mut dyn FnMut(&Path, bool) -> Probe,
) -> Option<LinkMatch> {
    if let Some((start, end, url)) = url_span_at(text, col) {
        return Some(LinkMatch {
            start,
            end,
            target: LinkTarget::Url(url),
        });
    }
    if !include_files {
        return None;
    }
    for candidate in file_candidates_at(text, col) {
        let Some((path, is_dir)) = resolve_candidate(&candidate, roots, probe) else {
            continue;
        };
        // A path can carry its line number instead of wearing it: only once
        // something has answered for the file is it worth reading the prose
        // beside it, and a directory has no line to land on.
        let line = match (candidate.line, is_dir) {
            (Some(line), _) => Some(line),
            (None, true) => None,
            (None, false) => location_after(text, candidate.end + 1),
        };
        return Some(LinkMatch {
            start: candidate.start,
            end: candidate.end,
            target: LinkTarget::File {
                path,
                line,
                column: candidate.column,
                is_dir,
            },
        });
    }
    None
}

/// The first of `candidate`'s possible paths that something answers for.
///
/// Roots are tried in order and the first [`Probe::Hit`] wins, which is what
/// makes the pane's own directory beat the repository root when both hold a
/// file by that name. A [`Probe::Unknown`] does not stop the walk — a later
/// root may already be cached — it just leaves the token unresolved for now.
pub(crate) fn resolve_candidate(
    candidate: &FileCandidate,
    roots: &LinkRoots,
    probe: &mut dyn FnMut(&Path, bool) -> Probe,
) -> Option<(PathBuf, bool)> {
    let require_file = candidate.require_file();
    candidate
        .paths(roots)
        .into_iter()
        .find_map(|path| match probe(&path, require_file) {
            Probe::Hit { is_dir } => Some((path, is_dir)),
            Probe::Miss | Probe::Unknown => None,
        })
}

/// A path-shaped token lifted out of the grid, before anything has checked
/// whether it points at something that exists. Splitting this out from the
/// lookup is what lets one pane answer from the filesystem and another answer
/// from a cache filled by a remote host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileCandidate {
    /// Column of the token's first character in the logical line.
    pub start: usize,
    /// Column of its last, inclusive.
    pub end: usize,
    /// The path exactly as it was written — relative, `~`-prefixed, absolute.
    pub path: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl FileCandidate {
    /// A line number is a claim about a file's *contents*, so a directory that
    /// happens to carry the name cannot answer for it. Without this,
    /// `localhost:8080` resolves the moment a `localhost/` directory exists.
    pub fn require_file(&self) -> bool {
        self.line.is_some()
    }

    /// Every path this token could mean, best guess first: absolute paths
    /// stand alone, relative ones are joined onto each root in turn.
    pub fn paths(&self, roots: &LinkRoots) -> Vec<PathBuf> {
        let Some(expanded) = expand_home(&self.path, roots) else {
            return Vec::new();
        };
        if expanded.is_empty() || roots.style.is_drive_relative(&expanded) {
            return Vec::new();
        }
        if let Some(rooted) = roots.style.rooted(&expanded) {
            return vec![PathBuf::from(rooted)];
        }
        let mut out: Vec<PathBuf> = Vec::new();
        for root in &roots.dirs {
            let joined = roots.style.join(root, &expanded);
            if !out.contains(&joined) {
                out.push(joined);
            }
        }
        out
    }

    /// Whether the token says for itself where it starts, rather than being
    /// measured from anywhere. [`Self::paths`] ignores the roots entirely for
    /// these, so a report about one must not name a directory as the place it
    /// was looked for.
    ///
    /// Takes the pane's own dialect for the same reason `paths` does: on a
    /// Windows client `/etc/hosts` printed by a Linux pane is rooted and
    /// `/etc/hosts` printed by a `cmd.exe` pane is not, and `Path` alone
    /// cannot tell those apart.
    pub fn is_rooted(&self, style: PathStyle) -> bool {
        self.path.starts_with('~') || style.rooted(&self.path).is_some()
    }

    /// Whether the token is written enough like a path to be worth telling the
    /// user about when nothing answers for it. A bare word is not — every
    /// modifier-click on ordinary output would raise a notification saying so.
    ///
    /// A backslash counts only where it separates directories. In a POSIX
    /// pane it is an escape or an ordinary filename character, so `foo\ bar`
    /// there is a word, not a path. A drive letter is one in any pane.
    pub fn looks_like_a_path(&self, style: PathStyle) -> bool {
        self.path.starts_with('~')
            || self.path.contains('/')
            || (style == PathStyle::Windows && self.path.contains('\\'))
            || drive_root(&self.path).is_some()
    }
}

/// Every path-shaped reading of the token under `col`, best first.
///
/// The syntactic half of file detection: everything that can be decided from
/// the text alone, with no filesystem behind it. One token can be read more
/// than one way because terminal output wraps paths in a small set of known
/// prefixes and suffixes — `--file=src/main.rs`, `+++ b/src/main.rs`,
/// `src@` from `ls -F` — and which reading is the real one is a question only
/// the filesystem can settle. So the ladder hands them all to the caller in
/// the order they are worth asking about, longest path first.
///
/// Deliberately a short ladder rather than every substring between every
/// separator: naming the prefixes costs a handful of readings where
/// enumerating pairs costs a probe per pair, on every hover.
pub(crate) fn file_candidates_at(text: &str, col: usize) -> Vec<FileCandidate> {
    let quoted = quoted_candidates_at(text, col);
    let Some((base, _, token)) = non_ws_token_at(text, col) else {
        return quoted;
    };
    let chars: Vec<char> = token.chars().collect();
    let mut out: Vec<FileCandidate> = Vec::new();
    for cut in left_cuts(&chars) {
        push_candidates(base, &chars, cut, col, &mut out);
    }
    // Longest path first: `/srv/app/log` beats the `/srv/app` inside it when
    // both exist, and a peeled reading only gets its turn once the one that
    // was actually written has missed.
    out.sort_by(|a, b| {
        b.path
            .chars()
            .count()
            .cmp(&a.path.chars().count())
            .then(a.start.cmp(&b.start))
    });
    // A quote is the one place a path is allowed a space, and the writer put
    // it there to say where the path starts and stops — so it is asked first.
    for (i, candidate) in quoted.into_iter().enumerate() {
        out.retain(|c| *c != candidate);
        out.insert(i, candidate);
    }
    out.truncate(MAX_FILE_CANDIDATES);
    out
}

/// The path a pair of quotes holds around `col`, when it has a space in it.
///
/// Whitespace ends a path everywhere else, and guessing where a path with
/// spaces stops in unquoted prose is how `see C:\Program Files for more`
/// would become a link to a folder that happens to exist. A quote is the
/// writer saying where it stops — `"C:\Program Files\app\app.exe"`,
/// `'/Users/me/My Docs/a.txt'` — so that is the only place a space is read as
/// part of a path.
///
/// What is inside still has to be written like a path: no space at either
/// end, a separator somewhere, and short enough to be one. A quoted sentence
/// that clears all that and still names nothing costs one reading, the same
/// as a single unquoted word — never one per word inside it.
///
/// A token with no space in it is left to the ordinary rules, which already
/// peel quotes off a path. A location may be written inside the quotes
/// (`"my file.rs:10:2"`) or straight after the closing one
/// (`"my file.rs":10:2`, `"my file.cs"(10,2)`).
fn quoted_candidates_at(text: &str, col: usize) -> Vec<FileCandidate> {
    let chars: Vec<char> = text.chars().collect();
    if col >= chars.len() {
        return Vec::new();
    }
    let Some((open, close)) = ['"', '\'', '`']
        .into_iter()
        .filter_map(|q| {
            let open = chars[..col].iter().rposition(|&c| c == q)?;
            let close = col + chars[col..].iter().position(|&c| c == q)?;
            (close > col).then_some((open, close))
        })
        // The innermost pair: `"it's '/a b/c'"` means the single-quoted one.
        .min_by_key(|&(open, close)| close - open)
    else {
        return Vec::new();
    };
    let inner = &chars[open + 1..close];
    let path_shaped = inner.len() <= MAX_QUOTED_PATH
        && inner.iter().any(|c| c.is_whitespace())
        && inner.first().is_some_and(|c| !c.is_whitespace())
        && inner.last().is_some_and(|c| !c.is_whitespace())
        && inner.iter().any(|&c| matches!(c, '/' | '\\'))
        && !inner.iter().any(|c| c.is_control());
    if !path_shaped {
        return Vec::new();
    }
    let written: String = inner.iter().collect();
    let mut location = split_file_location(&written);
    if location.line.is_none() {
        let after: String = chars[close + 1..]
            .iter()
            .take_while(|c| !c.is_whitespace())
            .collect();
        let after = after.trim_end_matches([':', ',', ';']);
        if !after.is_empty() {
            let outside = split_file_location(&format!("{written}{after}"));
            if outside.line.is_some() && outside.path == written {
                location = outside;
            }
        }
    }
    let mut readings = Vec::with_capacity(2);
    if location.literal_too && location.line.is_some() && location.path != written {
        readings.push((written.clone(), None, None));
    }
    readings.push((location.path, location.line, location.column));
    readings
        .into_iter()
        .map(|(path, line, column)| FileCandidate {
            start: open + 1,
            end: close - 1,
            path,
            line,
            column,
        })
        .collect()
}

/// The reading a click that resolved nothing should name.
///
/// The shortest one still written like a path, which is the part the user was
/// pointing at rather than the `--flag=` or `label:` wrapped around it. A
/// token that is not written like a path at all still comes back, so the
/// caller can decide to say nothing at all about it.
pub(crate) fn unresolved_candidate(
    text: &str,
    col: usize,
    style: PathStyle,
) -> Option<FileCandidate> {
    let candidates = file_candidates_at(text, col);
    candidates
        .iter()
        .rev()
        .find(|c| c.looks_like_a_path(style))
        .or_else(|| candidates.first())
        .cloned()
}

/// Where inside a token a path could start, besides the token's own start.
///
/// Each rule is applied to every cut it produces, so the prefixes stack the
/// way they do in real output: `git diff`'s `a/` behind a `--file=`, a label
/// behind that. The cap is what stops a separator-dense token from walking.
fn left_cuts(chars: &[char]) -> Vec<usize> {
    let mut cuts = vec![0usize];
    let mut next = 0;
    while next < cuts.len() && cuts.len() < MAX_FILE_CANDIDATES {
        let base = cuts[next];
        next += 1;
        let rest = &chars[base..];
        let mut offsets: Vec<usize> = Vec::new();
        // `--file=src/main.rs`, `PATH=/usr/bin`. The last `=` wins, so a value
        // that is itself an assignment keeps only its tail.
        if let Some(i) = rest.iter().rposition(|&c| c == '=') {
            offsets.push(i + 1);
        }
        // `note:src/main.rs`. Only a plain word may sit in front: one letter
        // is a Windows drive (`C:\src`), and a prefix carrying a `/` or a `.`
        // is more likely a path with a line number written onto it. What
        // follows has to be spelled like a path too, or `branch:main` and
        // `remote:origin` become links the moment the pane's directory holds
        // a `main/` or an `origin/`.
        //
        // CJK prose writes the label with no space and either colon:
        // `原型:/tmp/a.html`, `原型：/tmp/a.html`. A one-character label is
        // only a drive when it is an ASCII letter, so `图:/tmp/a.png` still
        // counts as a label.
        if let Some(i) = rest.iter().position(|&c| matches!(c, ':' | '：'))
            && (i >= 2 || (i == 1 && !rest[0].is_ascii()))
            && rest[0].is_alphabetic()
            && rest[..i]
                .iter()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '+'))
            && rest[i + 1..]
                .iter()
                .any(|c| matches!(c, '/' | '\\' | '.' | '~'))
        {
            offsets.push(i + 1);
        }
        // What `git diff` calls the two sides of a change.
        if matches!(rest.first(), Some('a' | 'b')) && rest.get(1) == Some(&'/') {
            offsets.push(2);
        }
        // A Windows drive glued onto whatever came before it: `保存到：C:\out`,
        // `图片已生成c:/Users/me/a.png`. CJK prose puts no space in front of a
        // path, and the label rule above cannot see these — the only ASCII
        // colon in the token is the drive's own. The letter must not be the
        // tail of an ASCII word, or `abc:/x` would read as drive `c:`.
        if let Some(i) = (1..rest.len()).find(|&i| {
            !rest[i - 1].is_ascii_alphanumeric()
                && rest[i].is_ascii_alphabetic()
                && rest.get(i + 1) == Some(&':')
                && matches!(rest.get(i + 2), Some('\\' | '/'))
        }) {
            offsets.push(i);
        }
        // A Markdown link's target, which is what an agent prints a file as:
        // `![chart](c:/out/chart.png)`, `[notes](docs/notes.md)`.
        if let Some(i) = rest.windows(2).position(|w| w == [']', '(']) {
            offsets.push(i + 2);
        }
        for offset in offsets {
            let cut = base + offset;
            if cut < chars.len() && !cuts.contains(&cut) {
                cuts.push(cut);
            }
        }
    }
    cuts
}

/// Turns one left cut into the readings it supports and files them in `out`.
fn push_candidates(
    base: usize,
    chars: &[char],
    cut: usize,
    col: usize,
    out: &mut Vec<FileCandidate>,
) {
    let mut start = cut;
    while chars.get(start).is_some_and(|&c| is_left_wrapper(c)) {
        start += 1;
    }
    let end = start + trim_token_end(&chars[start..]);
    if end <= start {
        return;
    }

    let mut ends = vec![end];
    // `ls -F` marks what it lists: `src@` is a symlink, `run*` an executable,
    // `sock=` a socket, `fifo|` a FIFO. The mark is not part of the name.
    if end - 1 > start && matches!(chars[end - 1], '@' | '*' | '=' | '|') {
        ends.push(end - 1);
    }

    for end in ends {
        let mut token: String = chars[start..end].iter().collect();
        let mut end = end - 1;
        let mut location = split_file_location(&token);
        // `foo.sh:` before `line 12`, and ripgrep's `path:` before a match.
        if location.line.is_none() && token.ends_with(':') {
            token.pop();
            end = end.saturating_sub(1);
            location = split_file_location(&token);
        }
        if location.path.is_empty() {
            continue;
        }
        if !(base + start..=base + end).contains(&col) {
            continue;
        }
        let mut readings = Vec::with_capacity(2);
        if location.literal_too {
            readings.push((token, None, None));
        }
        readings.push((location.path, location.line, location.column));
        for (path, line, column) in readings {
            let candidate = FileCandidate {
                start: base + start,
                end: base + end,
                path,
                line,
                column,
            };
            if !out.contains(&candidate) {
                out.push(candidate);
            }
        }
    }
}

pub(crate) fn url_span_at(text: &str, col: usize) -> Option<(usize, usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    if col >= chars.len() {
        return None;
    }
    if chars[col].is_whitespace() {
        return None;
    }
    let mut start = col;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < chars.len() && !chars[end + 1].is_whitespace() {
        end += 1;
    }
    let mut token: String = chars[start..=end].iter().collect();
    trim_trailing_punct(&mut token);

    const SCHEMES: [&str; 4] = ["https://", "http://", "file://", "ftp://"];
    if let Some(off) = SCHEMES.iter().filter_map(|s| token.find(s)).min() {
        start += token[..off].chars().count();
        token.drain(..off);
        if let Some(bad) = token.find(|c| !is_url_char(c)) {
            token.truncate(bad);
        }
        truncate_at_unbalanced_close(&mut token);
        trim_trailing_punct(&mut token);
        let end = start + token.chars().count() - 1;
        return (start..=end).contains(&col).then_some((start, end, token));
    }

    while token
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '(' | '[' | '<' | '\'' | '"' | '{'))
    {
        token.remove(0);
        start += 1;
    }
    trim_trailing_punct(&mut token);
    if token.starts_with("www.") && token.contains('.') {
        let end = start + token.chars().count() - 1;
        (start..=end)
            .contains(&col)
            .then(|| (start, end, format!("https://{token}")))
    } else {
        None
    }
}

fn non_ws_token_at(text: &str, col: usize) -> Option<(usize, usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    if col >= chars.len() || chars[col].is_whitespace() {
        return None;
    }

    let mut start = col;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < chars.len() && !chars[end + 1].is_whitespace() {
        end += 1;
    }
    Some((start, end, chars[start..=end].iter().collect()))
}

/// What can open a path without being part of it: the brackets and quotes
/// prose puts around one, and the box-drawing glyphs `tree` and friends draw
/// in front of one.
fn is_left_wrapper(c: char) -> bool {
    matches!(
        c,
        '(' | '[' | '<' | '{' | '\'' | '"' | '`' | '（' | '［' | '｛' | '《' | '「' | '『' | '【'
    ) || ('\u{2500}'..='\u{257F}').contains(&c)
}

/// Where the path stops and the sentence around it starts again, as an
/// exclusive char index.
fn trim_token_end(chars: &[char]) -> usize {
    let mut end = chars.len();
    while end > 0 {
        let strip = match chars[end - 1] {
            // A close that has an open of its own inside the token belongs to
            // the path: `report(1).pdf` is a filename, `(src/lib.rs)` is not.
            ')' => count_chars(&chars[..end], ')') > count_chars(&chars[..end], '('),
            ']' => count_chars(&chars[..end], ']') > count_chars(&chars[..end], '['),
            '}' => count_chars(&chars[..end], '}') > count_chars(&chars[..end], '{'),
            // `.`, `..` and `foo/.` are directories, not a sentence that ran
            // into the path.
            '.' => !matches!(
                end.checked_sub(2).and_then(|i| chars.get(i)),
                None | Some('.' | '/' | '\\')
            ),
            ',' | ';' | '\'' | '"' | '>' | '`' | '）' | '］' | '】' | '》' | '」' | '』' | '。'
            | '，' | '、' | '；' | '：' => true,
            _ => false,
        };
        if !strip {
            break;
        }
        end -= 1;
    }
    end
}

fn count_chars(chars: &[char], needle: char) -> usize {
    chars.iter().filter(|&&c| c == needle).count()
}

struct FileLocation {
    path: String,
    line: Option<u32>,
    column: Option<u32>,
    /// Whether the whole token is also worth reading as a filename.
    ///
    /// `backup(1)` and `main.ts(10)` are spelled identically, and only the
    /// filesystem knows which is which, so both readings go on the ladder.
    /// A `:10:2` suffix gets no such courtesy: a colon is illegal in a
    /// Windows filename and vanishingly rare in a POSIX one, and doubling
    /// the probes for the spelling every compiler uses is not worth it.
    literal_too: bool,
}

/// The path, and the `line:column` written onto the end of it.
///
/// Three spellings cover what build tools actually print: `main.rs:10:2`
/// (rustc, clang, ripgrep, eslint), `main.ts(10,2)` (tsc, MSVC, and every
/// compiler that grew up on Windows) and `main.rs#L10` (a permalink pasted
/// into a shell).
fn split_file_location(token: &str) -> FileLocation {
    if let Some(location) = paren_location(token) {
        return location;
    }
    if let Some(location) = anchor_location(token) {
        return location;
    }
    colon_location(token)
}

/// `main.ts(10,2)` and `main.ts(10)`.
fn paren_location(token: &str) -> Option<FileLocation> {
    let inner = token.strip_suffix(')')?;
    let open = inner.rfind('(')?;
    let (path, digits) = inner.split_at(open);
    let digits = &digits['('.len_utf8()..];
    if path.is_empty() || digits.is_empty() {
        return None;
    }
    let mut parts = digits.split(',').map(str::trim);
    let line = parts.next()?.parse().ok()?;
    let column = match parts.next() {
        Some(column) => Some(column.parse().ok()?),
        None => None,
    };
    parts.next().is_none().then_some(FileLocation {
        path: path.to_string(),
        line: Some(line),
        column,
        literal_too: true,
    })
}

/// `main.rs#L10`, and the `#L10-L20` a GitHub range permalink carries.
fn anchor_location(token: &str) -> Option<FileLocation> {
    let (path, anchor) = token.rsplit_once("#L")?;
    if path.is_empty() {
        return None;
    }
    let line = anchor.split('-').next()?.parse().ok()?;
    Some(FileLocation {
        path: path.to_string(),
        line: Some(line),
        column: None,
        literal_too: true,
    })
}

/// `main.rs:10:2` and `main.rs:10`.
fn colon_location(token: &str) -> FileLocation {
    let Some((prefix, last)) = strip_numeric_suffix(token) else {
        return FileLocation {
            path: token.to_string(),
            line: None,
            column: None,
            literal_too: false,
        };
    };
    if let Some((path, line)) = strip_numeric_suffix(prefix) {
        FileLocation {
            path: path.to_string(),
            line: Some(line),
            column: Some(last),
            literal_too: false,
        }
    } else {
        FileLocation {
            path: prefix.to_string(),
            line: Some(last),
            column: None,
            literal_too: false,
        }
    }
}

fn strip_numeric_suffix(token: &str) -> Option<(&str, u32)> {
    let (prefix, suffix) = token.rsplit_once(':')?;
    if prefix.is_empty() || suffix.is_empty() || !suffix.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let value = suffix.parse().ok()?;
    Some((prefix, value))
}

/// The line number written *next to* a path rather than onto it.
///
/// Python's traceback is why this exists: `File "handlers.py", line 214` puts
/// the number two tokens away, and landing on the file but not the line is
/// most of the way to useless in the one place where clicking a path is worth
/// the most. `bash`, `make` and `pytest` spell it the same way.
fn location_after(text: &str, from: usize) -> Option<u32> {
    /// Far enough to clear `", line ` and the number, and short enough that a
    /// `line` belonging to the next sentence is out of reach.
    const WINDOW: usize = 24;

    let chars: Vec<char> = text.chars().skip(from).take(WINDOW).collect();
    let at = |i: usize| chars.get(i).copied();
    let mut i = 0;
    // Whatever closed the path: the quote around it, the comma after it, the
    // colon `bash` puts there.
    while at(i)
        .is_some_and(|c| matches!(c, '"' | '\'' | ',' | ':' | ')' | ']') || c.is_whitespace())
    {
        i += 1;
    }
    let word: String = chars.iter().skip(i).take(4).collect();
    if !word.eq_ignore_ascii_case("line") {
        return None;
    }
    i += 4;
    // `lineage 5` is not a location.
    if !at(i).is_some_and(char::is_whitespace) {
        return None;
    }
    while at(i).is_some_and(char::is_whitespace) {
        i += 1;
    }
    let digits: String = chars
        .iter()
        .skip(i)
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// The token with a leading `~` turned into a real directory, still spelled
/// the pane's way — a string rather than a `PathBuf`, because deciding what is
/// absolute and how to join is the [`PathStyle`]'s job from here on and
/// `Path` would answer for the wrong machine.
pub(super) fn expand_home(path: &str, roots: &LinkRoots) -> Option<String> {
    let home = || {
        home_dir(roots.cwd(), roots.local_home, roots.style)
            .map(|home| home.to_string_lossy().into_owned())
    };
    if path == "~" {
        return home();
    }
    let rest = match roots.style {
        PathStyle::Posix => path.strip_prefix("~/"),
        PathStyle::Windows => path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")),
    };
    if let Some(rest) = rest {
        return home().map(|home| match roots.style {
            PathStyle::Posix => format!("{}/{rest}", home.trim_end_matches('/')),
            PathStyle::Windows => Path::new(&home).join(rest).to_string_lossy().into_owned(),
        });
    }
    Some(path.to_string())
}

/// The home `~` stands for, read out of the cwd where it can be and out of the
/// environment where it cannot.
///
/// The environment half only applies to a pane on this machine. Anywhere else
/// it would be guessing with our own answer: a cwd of `/srv/app` on a Linux
/// box says nothing about that box's home, and turning `~/.zshrc` into
/// `/Users/me/.zshrc` and asking the far side about it is how a link ends up
/// pointing at a file nobody meant.
fn home_dir(cwd: Option<&Path>, local_home: bool, style: PathStyle) -> Option<PathBuf> {
    if let Some(home) = cwd.and_then(|cwd| home_from_cwd(cwd, style)) {
        return Some(home);
    }
    if !local_home {
        return None;
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// The `/home/<user>` or `/Users/<user>` a POSIX cwd sits under.
///
/// Read off the string rather than `Path::components`, and keyed off the
/// pane's dialect rather than `cfg!(unix)`. The old spelling was gated to Unix
/// clients, which meant a Windows anterm could not say what `~` meant in *any*
/// pane it was looking at — `~/.zshrc` printed by a Linux host resolved on a
/// Mac and silently did not on a Windows box beside it.
///
/// A Windows-dialect cwd still gets no answer: nothing about `D:\Users\team`
/// says whose home it is, and the environment fallback in [`home_dir`] is both
/// available and right for the panes that spell paths that way.
fn home_from_cwd(cwd: &Path, style: PathStyle) -> Option<PathBuf> {
    if style != PathStyle::Posix {
        return None;
    }
    let cwd = cwd.to_str()?;
    let mut parts = cwd.strip_prefix('/')?.split('/').filter(|p| !p.is_empty());
    let base = parts.next()?;
    let user = parts.next()?;
    matches!(base, "Users" | "home").then(|| PathBuf::from(format!("/{base}/{user}")))
}

fn trim_trailing_punct(token: &mut String) {
    loop {
        let strip = match token.chars().next_back() {
            Some(')') => count_char(token, ')') > count_char(token, '('),
            Some(']') => count_char(token, ']') > count_char(token, '['),
            Some(
                '.' | ',' | ';' | ':' | '\'' | '"' | '>' | '）' | '］' | '】' | '》' | '」' | '。'
                | '，' | '；' | '：',
            ) => true,
            _ => false,
        };
        if !strip {
            return;
        }
        token.pop();
    }
}

fn count_char(s: &str, needle: char) -> usize {
    s.chars().filter(|&c| c == needle).count()
}

fn truncate_at_unbalanced_close(token: &mut String) {
    let mut parens = 0usize;
    let mut brackets = 0usize;
    for (i, c) in token.char_indices() {
        match c {
            '(' => parens += 1,
            '[' => brackets += 1,
            ')' if parens == 0 => {
                token.truncate(i);
                return;
            }
            ']' if brackets == 0 => {
                token.truncate(i);
                return;
            }
            ')' => parens -= 1,
            ']' => brackets -= 1,
            _ => {}
        }
    }
}

pub(crate) fn is_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '-' | '.'
                | '_'
                | '~'
                | ':'
                | '/'
                | '?'
                | '#'
                | '['
                | ']'
                | '@'
                | '!'
                | '$'
                | '&'
                | '\''
                | '('
                | ')'
                | '*'
                | '+'
                | ','
                | ';'
                | '='
                | '%'
        )
}
