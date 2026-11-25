//! History: the commits themselves, the refs pointing at them, and the lane
//! layout the graph is drawn from.
//!
//! The lane assignment lives here, not in the renderer, for two reasons. It is
//! a pure function over `(sha, parents)` and therefore the part of the graph
//! most worth testing exhaustively; and gpui re-runs `render` on every notify,
//! so an O(commits × lanes) pass in a paint closure would be burned every
//! frame for a result that only changes when the history does.

mod lanes;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;

use smallvec::SmallVec;

use lanes::row_span;
use super::RecordSplitter;
use super::diff::FileStatus;
use crate::host::Host;

pub use lanes::LaneAlloc;

/// Full hex object id. Kept as `String` rather than `[u8; 20]` because sha256
/// repositories exist and the extra allocation is noise next to the subject.
pub type Oid = String;

/// Lane index as assigned by the layout pass — the *true* column, before the
/// renderer folds anything past its width cap into an overflow column.
pub type Lane = u16;

/// Which palette entry a lane draws with. Equal to the lane it was created for
/// and never reassigned, which is what keeps a branch one colour for its whole
/// life: a branch holds the same lane from its tip until it is merged, because
/// the first parent inherits the lane in place and never migrates.
pub type ColorIdx = u16;

pub const GRAPH_PAGE: usize = 200;
pub const MAX_GRAPH_COMMITS: usize = 5_000;
pub const MAX_LANES: Lane = 32;
pub const MAX_REFS: usize = 2_000;
/// How many changed files one commit's detail view will hold. A vendored
/// dependency landing in a single commit is tens of thousands of paths, and
/// every one of them would become a row.
pub const MAX_COMMIT_FILES: usize = 1_000;
pub const MAX_SUBJECT_BYTES: usize = 512;
pub const MAX_BODY_BYTES: usize = 8 * 1024;
pub const MAX_LOG_BYTES: usize = 16 * 1024 * 1024;

/// Record separator for `log --pretty`. Deliberately not NUL: `git log -z`
/// already uses NUL between records, so a NUL field separator could only be
/// told apart by counting fields — and one NUL inside a commit message (git
/// objects allow it) would desynchronise the whole stream. RS and US cannot
/// occur in a sha, a refname, an ISO date or an address.
///
/// A commit *message* can still carry RS or US — nothing git accepts is out of
/// bounds there. The failure is contained, not eliminated: the body truncates
/// at the stray separator, `is_hex_oid` throws the tail away unless it is
/// deliberately shaped like a full record, and a deliberately shaped one can
/// fabricate at worst a bogus row in the graph — whose `git show` then fails.
/// Sealing that needs length-prefixed reads (`cat-file --batch`), a different
/// data path entirely.
pub const REC_SEP: u8 = 0x1e;
pub const FIELD_SEP: u8 = 0x1f;

/// A timestamp plus the author's own UTC offset, parsed from `%aI` / `%cI`.
///
/// `offset_minutes` is already subtracted out of `unix`, which is the field
/// everything currently renders from. Keeping the offset costs four bytes and
/// preserves the one thing the conversion to UTC throws away — what o'clock it
/// was where the commit was written. Nothing shows that yet; the parser is
/// simply not the place to lose it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OffsetTs {
    pub unix: i64,
    pub offset_minutes: i32,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Signature {
    pub name: String,
    pub email: String,
    pub at: OffsetTs,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum RefKind {
    /// Sorts last so the highest-priority chip wins a `max()`.
    Other,
    RemoteBranch,
    Tag,
    LocalBranch,
    Head,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RefDeco {
    pub kind: RefKind,
    /// `refs/heads/feature/x`
    pub full: String,
    /// `feature/x`
    pub short: String,
    /// Carried the `HEAD -> ` prefix in `%D`.
    pub is_head: bool,
    /// The full refname this branch tracks, where it tracks one. Only
    /// [`for_each_ref`] can fill it in — `%D` says nothing about upstreams —
    /// so a decoration parsed out of a `log` record always leaves it `None`.
    pub upstream: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Commit {
    pub oid: Oid,
    pub parents: SmallVec<[Oid; 2]>,
    pub author: Signature,
    pub committer: Signature,
    pub summary: String,
    pub body: String,
    pub refs: Vec<RefDeco>,
}

impl Commit {
    pub fn short(&self) -> &str {
        let n = self.oid.len().min(7);
        &self.oid[..n]
    }

    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }
}

/// One line inside a single row's band: from the row's top edge to its bottom.
///
/// Row-local on purpose. A model that described whole polylines across rows
/// could not emit a line until its far end arrived, so a long-lived branch
/// would stay invisible until the page holding its parent loaded — the bug
/// Zed's own graph has. Here a row is final the moment it is produced, which
/// is also what makes paging free of visual reflow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    /// Straight through the band without touching this row's node.
    Pass { lane: Lane, color: ColorIdx },
    /// Comes down from `from` on the top edge and ends at this row's node.
    In { from: Lane, color: ColorIdx },
    /// Leaves this row's node for `to` on the bottom edge.
    Out { to: Lane, color: ColorIdx },
}

impl Edge {
    pub fn color(self) -> ColorIdx {
        match self {
            Edge::Pass { color, .. } | Edge::In { color, .. } | Edge::Out { color, .. } => color,
        }
    }

    /// Paint order: pass-through lines first, so the node's own line lands on
    /// top of anything crossing behind it.
    pub fn paint_rank(&self) -> u8 {
        match self {
            Edge::Pass { .. } => 0,
            Edge::In { .. } => 1,
            Edge::Out { .. } => 2,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GraphRow {
    pub node: Lane,
    pub color: ColorIdx,
    /// 0 = root commit, 1 = ordinary, >1 = merge (>2 = octopus).
    pub parents: u8,
    pub edges: SmallVec<[Edge; 4]>,
}

/// Which refs the log is walked from.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum GraphScope {
    Head,
    /// HEAD plus its upstream — the default, matching VS Code.
    #[default]
    HeadAndUpstream,
    All,
    Refs(Vec<String>),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommitPage {
    pub commits: Vec<Commit>,
    /// Same length as `commits`.
    pub rows: Vec<GraphRow>,
    pub max_lanes: Lane,
    pub scope: GraphScope,
    pub requested: usize,
    /// git returned fewer than asked for, so this is the end of history.
    pub complete: bool,
    pub truncated_lanes: bool,
    /// Lanes still open past the last row — drawn as fading stubs so a page
    /// boundary does not read as a row of root commits.
    pub open_lanes: Vec<Lane>,
}

/// The eleven fields, in order: sha, parents, author name/email/date,
/// committer name/email/date, decorations, subject, body.
pub const LOG_PRETTY: &str =
    "--pretty=format:%x1e%H%x1f%P%x1f%an%x1f%ae%x1f%aI%x1f%cn%x1f%ce%x1f%cI%x1f%D%x1f%s%x1f%b";

const LOG_FIELDS: usize = 11;

pub const REF_FORMAT: &str = "--format=%(objectname)%x1f%(refname)%x1f%(refname:short)%x1f%(upstream)%x1f%(HEAD)%x1f%(objecttype)%x1f%(*objectname)";

/// What [`parse_log`] read, and whether it read all of it.
pub struct ParsedLog {
    pub commits: Vec<Commit>,
    /// The stream was cut short — by [`MAX_LOG_BYTES`], or by a record past
    /// `MAX_RECORD` being dropped whole. The caller must not present the
    /// commits as "all of history": git returned more than was parsed.
    pub truncated: bool,
}

/// Parses the output of the `log` invocation [`LOG_PRETTY`] belongs to.
///
/// Records are split on RS and fields on US. Fields are taken with `splitn`, so
/// the body — the only field that can contain anything at all — absorbs every
/// separator past the tenth instead of shifting the parse.
pub fn parse_log(stdout: &[u8]) -> ParsedLog {
    let mut commits = Vec::new();
    let mut used = 0usize;
    let mut clipped = false;
    let mut on_record = |record: &[u8]| {
        used = used.saturating_add(record.len());
        if used > MAX_LOG_BYTES {
            clipped = true;
            return;
        }
        if let Some(commit) = parse_record(record) {
            commits.push(commit);
        }
    };
    let mut split = RecordSplitter::new(REC_SEP);
    split.push(stdout, &mut on_record);
    let dropped = split.finish(&mut on_record);
    ParsedLog {
        commits,
        truncated: clipped || dropped > 0,
    }
}

fn parse_record(record: &[u8]) -> Option<Commit> {
    let text = String::from_utf8_lossy(record);
    let mut fields = text.splitn(LOG_FIELDS, FIELD_SEP as char);
    let oid = fields.next()?;
    // The stream opens with an empty record (nothing precedes the first RS),
    // and a format drift would otherwise turn one bad record into a page of
    // nonsense. Anything that is not a sha is not a commit.
    if !is_hex_oid(oid) {
        return None;
    }
    let parents = fields
        .next()?
        .split_ascii_whitespace()
        .map(str::to_string)
        .collect();
    let author_name = fields.next()?;
    let author_email = fields.next()?;
    let author_at = fields.next()?;
    let committer_name = fields.next()?;
    let committer_email = fields.next()?;
    let committer_at = fields.next()?;
    let deco = fields.next()?;
    let subject = fields.next()?;
    // git joins records with a newline, so the last field carries it.
    let body = fields.next()?.trim_end_matches(['\n', '\r']);

    Some(Commit {
        oid: oid.to_string(),
        parents,
        author: signature(author_name, author_email, author_at),
        committer: signature(committer_name, committer_email, committer_at),
        summary: clip(subject, MAX_SUBJECT_BYTES).to_string(),
        body: clip(body, MAX_BODY_BYTES).to_string(),
        refs: parse_deco(deco),
    })
}

fn signature(name: &str, email: &str, at: &str) -> Signature {
    Signature {
        name: name.to_string(),
        email: email.to_string(),
        // A commit whose `%aI` will not parse is not worth dropping the commit
        // over; it loses its timestamp and keeps everything else.
        at: parse_iso8601(at).unwrap_or(OffsetTs {
            unix: 0,
            offset_minutes: 0,
        }),
    }
}

fn is_hex_oid(text: &str) -> bool {
    (4..=64).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Truncates to at most `max` bytes without splitting a character.
fn clip(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `%D` with `--decorate=full`: `HEAD -> refs/heads/main, tag: refs/tags/v1,
/// refs/remotes/origin/main`.
fn parse_deco(text: &str) -> Vec<RefDeco> {
    let mut out = Vec::new();
    for piece in text.split(',') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let (is_head, name) = match piece.strip_prefix("HEAD -> ") {
            Some(rest) => (true, rest.trim()),
            None => (false, piece),
        };
        // `--decorate=full` spells refnames out in full but still marks tags
        // with a `tag: ` prefix, so both have to come off.
        let name = name.strip_prefix("tag: ").unwrap_or(name).trim();
        if name == "HEAD" {
            out.push(RefDeco {
                kind: RefKind::Head,
                full: "HEAD".to_string(),
                short: "HEAD".to_string(),
                is_head: true,
                upstream: None,
            });
            continue;
        }
        if let Some(deco) = ref_deco(name, is_head) {
            out.push(deco);
        }
    }
    out
}

fn ref_deco(full: &str, is_head: bool) -> Option<RefDeco> {
    let (kind, short) = if let Some(rest) = full.strip_prefix("refs/heads/") {
        (RefKind::LocalBranch, rest)
    } else if let Some(rest) = full.strip_prefix("refs/remotes/") {
        (RefKind::RemoteBranch, rest)
    } else if let Some(rest) = full.strip_prefix("refs/tags/") {
        (RefKind::Tag, rest)
    } else {
        (RefKind::Other, full.strip_prefix("refs/").unwrap_or(full))
    };
    if short.is_empty() {
        return None;
    }
    Some(RefDeco {
        kind,
        full: full.to_string(),
        short: short.to_string(),
        is_head,
        upstream: None,
    })
}

/// Parses the output of the `for-each-ref` invocation [`REF_FORMAT`] belongs
/// to, keyed by the commit each ref ultimately points at.
pub fn parse_refs(stdout: &[u8]) -> HashMap<Oid, Vec<RefDeco>> {
    let mut out: HashMap<Oid, Vec<RefDeco>> = HashMap::new();
    let text = String::from_utf8_lossy(stdout);
    for line in text.lines().take(MAX_REFS) {
        let mut fields = line.split(FIELD_SEP as char);
        let object = fields.next().unwrap_or_default().trim();
        let full = fields.next().unwrap_or_default().trim();
        // `%(refname:short)` is skipped in favour of stripping the prefix here,
        // so a ref named the same way from `%D` and from here reads identically
        // in the UI. `%(objecttype)` is asked for to keep the format one line
        // rather than two; only `%(*objectname)` below acts on it.
        let _short = fields.next();
        let upstream = fields.next().unwrap_or_default().trim();
        let head = fields.next().unwrap_or_default().trim();
        let _kind = fields.next();
        let peeled = fields.next().unwrap_or_default().trim();
        // `%(*objectname)` is empty unless this is an annotated tag. When it is
        // not, the chip belongs on the commit rather than on the tag object,
        // which is the only reason the field is in the format.
        let target = if peeled.is_empty() { object } else { peeled };
        if full.is_empty() || !is_hex_oid(target) {
            continue;
        }
        if let Some(mut deco) = ref_deco(full, head == "*") {
            deco.upstream = (!upstream.is_empty()).then(|| upstream.to_string());
            out.entry(target.to_string()).or_default().push(deco);
        }
    }
    out
}

pub fn for_each_ref(host: &dyn Host, root: &Path) -> HashMap<Oid, Vec<RefDeco>> {
    let count = format!("--count={MAX_REFS}");
    let args = [
        "for-each-ref",
        "--sort=-committerdate",
        &count,
        REF_FORMAT,
        "refs/heads",
        "refs/remotes",
        "refs/tags",
    ];
    match host.git(root, &args) {
        Ok(out) if out.success() => parse_refs(&out.stdout),
        _ => HashMap::new(),
    }
}

/// The local branch names, one per line, in git's own refname order.
///
/// `for-each-ref` rather than `branch`: no porcelain warnings, no column
/// layout, and one name per line whatever the user's config says. It is a
/// separate call from [`for_each_ref`] because that one groups by the commit a
/// ref points at, which is the wrong shape for a list of branches — the
/// switcher wants every branch, including the ones sharing a tip.
pub fn local_branches(host: &dyn Host, root: &Path) -> Vec<String> {
    let count = format!("--count={MAX_REFS}");
    let args = [
        "for-each-ref",
        &count,
        "--format=%(refname:short)",
        "refs/heads",
    ];
    match host.git(root, &args) {
        Ok(out) if out.success() => parse_branch_names(&out.stdout),
        _ => Vec::new(),
    }
}

pub fn parse_branch_names(stdout: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .take(MAX_REFS)
        .map(str::to_string)
        .collect()
}

/// One commit's metadata, for a detail view that does not already have it.
///
/// A commit that is on screen in the graph is already a [`Commit`] in
/// [`CommitPage::commits`], and the caller is expected to hand that over
/// instead of paying for this. What is left is the case the page cannot
/// answer: a commit reached from a parent link, or from anywhere outside the
/// window the graph happens to be holding.
pub fn load_commit(host: &dyn Host, root: &Path, rev: &str) -> Option<Commit> {
    if !is_rev(rev) {
        return None;
    }
    let args = [
        "-c",
        "log.showSignature=false",
        "show",
        "--no-patch",
        // Without it `%D` prints short names, and `parse_deco` reads full
        // ones — every chip would come back as `RefKind::Other`.
        "--decorate=full",
        "--no-color",
        LOG_PRETTY,
        rev,
    ];
    let out = host.git(root, &args).ok()?;
    if !out.success() {
        return None;
    }
    parse_log(&out.stdout).commits.into_iter().next()
}

/// One path a commit touched, with the line counts beside it.
///
/// The counts are `Option` rather than `0` because "git did not say" and
/// "nothing changed" are different answers: a binary file reports neither, and
/// a pure rename reports `0 0`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommitFile {
    /// Repository-relative, and for a rename the *new* name.
    pub path: String,
    /// Where a rename or a copy came from.
    pub orig_path: Option<String>,
    pub status: FileStatus,
    pub added: Option<u32>,
    pub removed: Option<u32>,
    pub binary: bool,
}

/// The paths one commit touched, against its first parent.
///
/// Two commands rather than one, because git will take `--numstat` and
/// `--name-status` together and then quietly drop the numstat half — measured
/// on 2.50.1, where the combined `-z` stream comes back as pure name-status.
/// So they are run separately and joined on the path.
///
/// `log -1 --first-parent`, *not* `diff-tree -m --first-parent`: `diff-tree`
/// does not honour `--first-parent` as a narrowing of a merge. On the same git
/// it emits one diff per parent and concatenates them, so a two-parent merge
/// comes back with a file list twice as long as the merge really is. `log` is
/// also exactly how [`DiffSource::Commit`](super::diff::DiffSource) walks the
/// patch, which is what makes this list and the overlay's cards agree file for
/// file — and it needs no `--root`, because `log` shows a root commit's
/// contents as additions without being asked.
pub fn commit_files(host: &dyn Host, root: &Path, rev: &str) -> Option<Vec<CommitFile>> {
    let numstat = commit_diff(host, root, rev, "--numstat")?;
    let name_status = commit_diff(host, root, rev, "--name-status")?;
    Some(join_commit_files(&numstat, &name_status))
}

fn commit_diff(host: &dyn Host, root: &Path, rev: &str, what: &str) -> Option<Vec<u8>> {
    if !is_rev(rev) {
        return None;
    }
    let args = [
        "-c",
        "log.showSignature=false",
        // Without it a non-ASCII path comes back wrapped in quotes with its
        // bytes spelled as C octal escapes, and nothing here decodes those.
        "-c",
        "core.quotePath=false",
        "log",
        "-1",
        "--format=",
        "--first-parent",
        "--no-color",
        "-z",
        what,
        "--find-renames",
        rev,
    ];
    let out = host.git(root, &args).ok()?;
    out.success().then_some(out.stdout)
}

/// A rev the caller made up is still a rev git will be handed, so anything
/// that could be read as an option is refused before it gets there.
fn is_rev(rev: &str) -> bool {
    !rev.is_empty() && !rev.starts_with('-') && !rev.contains(|c: char| c.is_control())
}

fn records(stdout: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut on_record = |record: &[u8]| out.push(String::from_utf8_lossy(record).into_owned());
    let mut split = RecordSplitter::new(0);
    split.push(stdout, &mut on_record);
    // A dropped record here is a >1 MiB *pathname* — losing that one row from
    // a commit's file list is the same answer `MAX_COMMIT_FILES` already gives
    // for lists that are merely long.
    let _ = split.finish(&mut on_record);
    out
}

/// `-z --numstat`: `<added>\t<removed>\t<path>\0` per file — except for a
/// rename or a copy, where the third field is *empty* and the old and the new
/// path follow as two records of their own. A binary file reports `-\t-`.
fn parse_numstat(stdout: &[u8]) -> HashMap<String, (Option<u32>, Option<u32>, bool)> {
    let mut out = HashMap::new();
    let records = records(stdout);
    let mut at = 0usize;
    while at < records.len() && out.len() < MAX_COMMIT_FILES {
        let record = &records[at];
        at += 1;
        let mut fields = record.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let binary = added == "-" && removed == "-";
        let counts = (added.parse::<u32>().ok(), removed.parse::<u32>().ok());
        let path = if path.is_empty() {
            let new = records.get(at + 1).cloned();
            at += 2;
            match new {
                Some(new) => new,
                // Truncated mid-rename. Nothing else can be read from here.
                None => break,
            }
        } else {
            path.to_string()
        };
        out.insert(path, (counts.0, counts.1, binary));
    }
    out
}

/// `-z --name-status`: `<status>\0<path>\0`, and `R<score>\0<old>\0<new>\0`
/// for the two statuses that name two paths.
fn parse_name_status(stdout: &[u8]) -> Vec<(String, Option<String>, FileStatus)> {
    let mut out = Vec::new();
    let records = records(stdout);
    let mut at = 0usize;
    while at < records.len() && out.len() < MAX_COMMIT_FILES {
        let code = records[at].trim().to_string();
        at += 1;
        let two_paths = matches!(code.as_bytes().first(), Some(b'R' | b'C'));
        let taken = 1 + usize::from(two_paths);
        let Some(paths) = records.get(at..at + taken) else {
            break;
        };
        at += taken;
        let Some(status) = file_status(&code) else {
            continue;
        };
        match paths {
            [path] => out.push((path.clone(), None, status)),
            [old, new] => out.push((new.clone(), Some(old.clone()), status)),
            _ => {}
        }
    }
    out
}

fn file_status(code: &str) -> Option<FileStatus> {
    match code.as_bytes().first()? {
        b'A' => Some(FileStatus::Added),
        b'M' => Some(FileStatus::Modified),
        b'D' => Some(FileStatus::Deleted),
        b'R' => Some(FileStatus::Renamed),
        b'C' => Some(FileStatus::Copied),
        b'T' => Some(FileStatus::TypeChanged),
        // `X` is git's own "unknown"; `B` only appears under
        // `--break-rewrites`, which nothing here passes.
        b'U' => Some(FileStatus::Unmerged),
        _ => None,
    }
}

/// Joins the two streams on the path.
///
/// `--name-status` is the spine: it carries the letter every row is drawn
/// from, and it is in git's own order. `--numstat` only contributes counts, so
/// losing it costs the numbers and nothing else. Losing the other way round is
/// worse — a path with counts and no letter would vanish — so anything left
/// over is appended rather than dropped.
pub fn join_commit_files(numstat: &[u8], name_status: &[u8]) -> Vec<CommitFile> {
    let mut counts = parse_numstat(numstat);
    let named = parse_name_status(name_status);
    let mut out: Vec<CommitFile> = Vec::with_capacity(named.len());
    for (path, orig_path, status) in named {
        let (added, removed, binary) = counts.remove(&path).unwrap_or((None, None, false));
        out.push(CommitFile {
            path,
            orig_path,
            status,
            added,
            removed,
            binary,
        });
    }
    let mut leftover: Vec<_> = counts.into_iter().collect();
    // A `HashMap` has no order to preserve, and a file list that reshuffles
    // itself between two reads of the same commit would be worse than a
    // list that is merely not in git's order.
    leftover.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, (added, removed, binary)) in leftover {
        if out.len() >= MAX_COMMIT_FILES {
            break;
        }
        out.push(CommitFile {
            path,
            orig_path: None,
            status: FileStatus::Modified,
            added,
            removed,
            binary,
        });
    }
    out
}

/// Loads the newest `count` commits of `scope` and lays them out.
///
/// Paging is a bigger `-n`, never `--skip`. `--skip=M` walks and discards M
/// commits every time, and any ref that moves between two pages shifts the
/// window so page two no longer continues page one. Re-walking is O(n) either
/// way, the layout is deterministic, so a larger page reproduces the previous
/// one as its prefix and nothing on screen moves.
pub fn load_page(
    host: &dyn Host,
    root: &Path,
    scope: &GraphScope,
    count: usize,
) -> Option<CommitPage> {
    let count = count.clamp(1, MAX_GRAPH_COMMITS);
    let revs = scope_revs(host, root, scope);
    if revs.is_empty() {
        // An unborn HEAD. Not a failure: there is simply no history yet.
        return Some(CommitPage {
            commits: Vec::new(),
            rows: Vec::new(),
            max_lanes: 0,
            scope: scope.clone(),
            requested: count,
            complete: true,
            truncated_lanes: false,
            open_lanes: Vec::new(),
        });
    }

    let n = count.to_string();
    let mut args = vec![
        "-c",
        // Verifying signatures on every commit costs more than everything else
        // in this command put together, and the graph never shows the result.
        "log.showSignature=false",
        "log",
        // Not `--date-order`: the layout needs every parent to come after all
        // of its children, and dates do not guarantee that. A rebase or a
        // cherry-pick across timezones is enough to invert a pair.
        "--topo-order",
        "--decorate=full",
        "--no-color",
        LOG_PRETTY,
        "-n",
        &n,
    ];
    args.extend(revs.iter().map(String::as_str));

    // Buffered rather than streamed on purpose. `Host::git_lines` splits on
    // newlines, which would chop these RS-delimited records apart and leave the
    // caller to glue them back together, and `Host::git` is byte-exact over the
    // control protocol's base64. 5000 commits is a few MB, which that carries
    // fine. If it ever stops being fine the fix is a raw byte stream on `Host`,
    // and that one does need a control protocol bump.
    let out = host.git(root, &args).ok()?;
    if !out.success() {
        return None;
    }
    let parsed = parse_log(&out.stdout);
    let mut commits = parsed.commits;
    // "End of history" needs both halves: git answered with fewer than asked
    // for, *and* the parse read everything git answered with. A stream cut at
    // `MAX_LOG_BYTES` also has fewer commits than `count` — calling that
    // complete would freeze paging on a truncated graph.
    let complete = !parsed.truncated && commits.len() < count;

    let page: Vec<(Oid, SmallVec<[Oid; 2]>)> = commits
        .iter()
        .map(|c| (c.oid.clone(), c.parents.clone()))
        .collect();
    let mut alloc = LaneAlloc::new();
    let mut rows = Vec::with_capacity(page.len());
    alloc.push(&page, &mut rows);
    let max_lanes = rows.iter().map(row_span).max().unwrap_or(0);

    let mut by_oid = for_each_ref(host, root);
    for commit in &mut commits {
        if let Some(extra) = by_oid.remove(&commit.oid) {
            for deco in extra {
                if !commit.refs.iter().any(|r| r.full == deco.full) {
                    commit.refs.push(deco);
                }
            }
        }
        // Highest priority first, so a row that has space for one chip can take
        // the first and count the rest.
        commit.refs.sort_by(|a, b| {
            b.is_head
                .cmp(&a.is_head)
                .then_with(|| b.kind.cmp(&a.kind))
                .then_with(|| a.short.cmp(&b.short))
        });
    }

    Some(CommitPage {
        commits,
        rows,
        max_lanes,
        scope: scope.clone(),
        requested: count,
        complete,
        truncated_lanes: alloc.truncated(),
        open_lanes: alloc.open_lanes(),
    })
}

/// The revs to walk for a scope.
///
/// Every symbolic name resolves to a sha first. Paging re-runs the walk with a
/// larger `-n`, and a symbolic `HEAD` or branch name would let a commit pushed
/// between the two runs change where page two starts — the second page would
/// no longer be a superset of the first, which is the one thing paging here
/// relies on. (`--all` cannot be pinned; that scope accepts the reflow.)
/// A name that no longer resolves — a deleted branch, an unborn HEAD — simply
/// contributes nothing, which reads as "no history" rather than as a failure.
fn scope_revs(host: &dyn Host, root: &Path, scope: &GraphScope) -> Vec<String> {
    match scope {
        GraphScope::Head => rev(host, root, "HEAD^{commit}").into_iter().collect(),
        GraphScope::All => vec!["--all".to_string()],
        GraphScope::Refs(refs) => refs
            .iter()
            // A refname cannot begin with `-`, so anything that does is
            // either a mistake or an option smuggled in through a scope.
            .filter(|r| !r.is_empty() && !r.starts_with('-'))
            .filter_map(|r| rev(host, root, &format!("{r}^{{commit}}")))
            .collect(),
        GraphScope::HeadAndUpstream => {
            let mut revs = Vec::new();
            if let Some(head) = rev(host, root, "HEAD^{commit}") {
                revs.push(head);
            }
            if let Some(upstream) = rev(host, root, "@{upstream}^{commit}")
                && !revs.contains(&upstream)
            {
                revs.push(upstream);
            }
            revs
        }
    }
}

fn rev(host: &dyn Host, root: &Path, spec: &str) -> Option<String> {
    let out = super::git(host, root, &["rev-parse", "--verify", "--quiet", spec])?;
    let sha = out.trim();
    is_hex_oid(sha).then(|| sha.to_string())
}

/// `%aI` is strict ISO 8601: `2026-08-09T14:03:11+08:00`, or `Z` for UTC.
///
/// Hand-rolled because the workspace carries neither `chrono` nor `time`, and
/// thirty lines of arithmetic is a poor reason to add a dependency tree to a
/// crate the headless server also builds.
pub(crate) fn parse_iso8601(text: &str) -> Option<OffsetTs> {
    let b = text.as_bytes();
    if !text.is_ascii() || b.len() < 19 {
        return None;
    }
    if b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    if b[10] != b'T' && b[10] != b't' && b[10] != b' ' {
        return None;
    }
    let year: i64 = text[0..4].parse().ok()?;
    let month: u32 = text[5..7].parse().ok()?;
    let day: u32 = text[8..10].parse().ok()?;
    let hour: i64 = text[11..13].parse().ok()?;
    let minute: i64 = text[14..16].parse().ok()?;
    // 60 is a leap second, which git will never emit but which is legal.
    let second: i64 = text[17..19].parse().ok()?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let offset_minutes = parse_offset(&text[19..])?;
    let unix = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
        - i64::from(offset_minutes) * 60;
    Some(OffsetTs {
        unix,
        offset_minutes,
    })
}

fn parse_offset(text: &str) -> Option<i32> {
    if text.is_empty() || text == "Z" || text == "z" {
        return Some(0);
    }
    let (sign, rest) = match text.as_bytes()[0] {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => return None,
    };
    let (hours, minutes) = match rest.len() {
        5 if rest.as_bytes()[2] == b':' => (&rest[0..2], &rest[3..5]),
        4 => (&rest[0..2], &rest[2..4]),
        2 => (rest, "0"),
        _ => return None,
    };
    let hours: i32 = hours.parse().ok()?;
    let minutes: i32 = minutes.parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 60 + minutes))
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days since 1970-01-01, by Hinnant's era algorithm. Shifting the year to
/// start in March is what keeps the leap rules out of the code: a 400-year era
/// is exactly 146097 days, so every correction collapses into a division.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted = i64::from((month + 9) % 12);
    let day_of_year = (153 * shifted + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
