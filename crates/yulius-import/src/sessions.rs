//! Claude Code and Codex session discovery (SPEC-001 IMP-003, AC-IMP-3).
//!
//! A session belongs to the repository when its first recorded working directory equals or is
//! inside one of the repository's checkouts (root or worktree); a Codex session also belongs when
//! its recorded `git.repository_url` names the same GitHub repository. It is included when its
//! last timestamp is at or after the window start. Transcripts are read with bounds: at most
//! 64 MiB per file, lines over 4 MiB skipped; a truncated file's final timestamps come from its
//! last 4 MiB.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::fs::{self, DirEntry};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;
use yulius_contracts::import::{
    ImportHarness, MAX_TITLE_CHARS, is_safe_file_name, is_safe_session_id,
};

use crate::git::{GitHubRemote, parse_github_remote};
use crate::jsonl::{JsonlReader, Line, MAX_LINE_BYTES, tail_lines};
use crate::time::{format_millis, parse_rfc3339_millis, system_time_millis};

/// A transcript file of a discovered session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFile {
    /// Upload name (`sessions/<harness>/<id>.jsonl`, subagents under `sessions/claude/<id>/`).
    pub name: String,
    /// Local path of the live transcript.
    pub path: PathBuf,
    /// Size in bytes at discovery time.
    pub size: u64,
}

/// A session opened in the repository within the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredSession {
    /// Recording harness.
    pub harness: ImportHarness,
    /// Harness session identifier.
    pub session_id: String,
    /// First real user message (at most 80 characters) or a generated fallback.
    pub title: String,
    /// Recorded working directory.
    pub cwd: PathBuf,
    /// `cwd` relative to the most specific matching checkout; `None` at a checkout root or when
    /// matched only by repository URL.
    pub cwd_relative: Option<String>,
    /// First timestamp (RFC 3339 UTC).
    pub started_at: String,
    /// Last timestamp (RFC 3339 UTC).
    pub ended_at: String,
    /// Branch recorded at session start.
    pub git_branch: Option<String>,
    /// Main transcript first, then auxiliary transcripts.
    pub files: Vec<SessionFile>,
    /// Lines skipped as malformed or over the line bound.
    pub skipped_lines: usize,
    /// `true` when reading stopped at the per-file byte bound.
    pub truncated: bool,
}

impl DiscoveredSession {
    /// Sum of the transcript file sizes.
    #[must_use]
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }
}

/// Finds Claude Code sessions under `<claude_dir>/projects/*/<sessionId>.jsonl` that belong to
/// `repo_paths` and were active at or after `since`. Nested `<sessionId>/subagents/*.jsonl`
/// transcripts are attached to their parent session, never reported as sessions. Unreadable or
/// unrecognizable files are skipped. Results are sorted by start time.
///
/// # Errors
/// Only when the `projects` directory exists but cannot be listed.
pub fn discover_claude_sessions(
    claude_dir: &Path,
    repo_paths: &[PathBuf],
    since: SystemTime,
) -> io::Result<Vec<DiscoveredSession>> {
    let Some(projects) = read_dir_if_exists(&claude_dir.join("projects"))? else {
        return Ok(Vec::new());
    };
    let matcher = RepoMatcher::new(repo_paths);
    let since_ms = system_time_millis(since);
    let mut found = BTreeMap::new();
    for project in projects.flatten() {
        if !project.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(entries) = fs::read_dir(project.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(session) = claude_session(&project.path(), &entry, &matcher, since_ms) {
                insert_latest(&mut found, session);
            }
        }
    }
    Ok(sorted(found))
}

/// Finds Codex sessions under `<codex_home>/sessions/YYYY/MM/DD/rollout-*.jsonl` that belong to
/// `repo_paths` or to the GitHub repository `remote_url`, active at or after `since`. Files whose
/// first line is not a `session_meta` record are skipped. Results are sorted by start time.
///
/// # Errors
/// Only when the `sessions` directory exists but cannot be listed.
pub fn discover_codex_sessions(
    codex_home: &Path,
    repo_paths: &[PathBuf],
    remote_url: &str,
    since: SystemTime,
) -> io::Result<Vec<DiscoveredSession>> {
    let root = codex_home.join("sessions");
    if read_dir_if_exists(&root)?.is_none() {
        return Ok(Vec::new());
    }
    let matcher = RepoMatcher::new(repo_paths);
    let remote = parse_github_remote(remote_url);
    let since_ms = system_time_millis(since);
    let mut files = Vec::new();
    collect_rollouts(&root, 3, &mut files);
    files.sort();
    let mut found = BTreeMap::new();
    for path in files {
        if let Some(session) = codex_session(&path, &matcher, remote.as_ref(), since_ms) {
            insert_latest(&mut found, session);
        }
    }
    Ok(sorted(found))
}

fn read_dir_if_exists(path: &Path) -> io::Result<Option<fs::ReadDir>> {
    match fs::read_dir(path) {
        Ok(entries) => Ok(Some(entries)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Keeps one session per identifier: the one active last.
fn insert_latest(found: &mut BTreeMap<String, DiscoveredSession>, session: DiscoveredSession) {
    match found.get(&session.session_id) {
        Some(existing) if existing.ended_at >= session.ended_at => {}
        _ => {
            found.insert(session.session_id.clone(), session);
        }
    }
}

fn sorted(found: BTreeMap<String, DiscoveredSession>) -> Vec<DiscoveredSession> {
    let mut sessions: Vec<DiscoveredSession> = found.into_values().collect();
    sessions.sort_by(|a, b| {
        (a.started_at.as_str(), a.session_id.as_str())
            .cmp(&(b.started_at.as_str(), b.session_id.as_str()))
    });
    sessions
}

/// `true` when the file was modified before the window (its content cannot be newer).
fn modified_before(entry_path: &Path, since_ms: i64) -> bool {
    fs::metadata(entry_path)
        .and_then(|meta| meta.modified())
        .is_ok_and(|modified| system_time_millis(modified) < since_ms)
}

fn file_size(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()
        .filter(fs::Metadata::is_file)
        .map(|meta| meta.len())
}

/// Matches recorded working directories against checkout roots.
struct RepoMatcher {
    /// Lexical and canonical forms, most specific (deepest) first.
    roots: Vec<PathBuf>,
}

impl RepoMatcher {
    fn new(paths: &[PathBuf]) -> Self {
        let mut roots = Vec::new();
        for path in paths {
            if is_plain_absolute(path) {
                roots.push(path.clone());
            }
            if let Ok(canonical) = fs::canonicalize(path) {
                roots.push(canonical);
            }
        }
        roots.sort_by(|a, b| {
            Reverse(a.components().count())
                .cmp(&Reverse(b.components().count()))
                .then_with(|| a.cmp(b))
        });
        roots.dedup();
        Self { roots }
    }

    /// `Some(relative)` when `cwd` is inside a checkout; the relative part is `None` at a root.
    fn locate(&self, cwd: &Path) -> Option<Option<String>> {
        let mut candidates = Vec::new();
        if is_plain_absolute(cwd) {
            candidates.push(cwd.to_path_buf());
        }
        if cwd.is_absolute()
            && let Ok(canonical) = fs::canonicalize(cwd)
        {
            candidates.push(canonical);
        }
        candidates.iter().find_map(|candidate| {
            self.roots
                .iter()
                .find_map(|root| candidate.strip_prefix(root).ok())
                .map(relative_string)
        })
    }
}

/// Absolute without `.` or `..` components, so lexical prefix checks are meaningful.
fn is_plain_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, Component::CurDir | Component::ParentDir))
}

fn relative_string(rest: &Path) -> Option<String> {
    let parts: Vec<String> = rest
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Text that is not a real user prompt (harness wrappers, commands, reminders, interrupts).
const SKIPPED_PREFIXES: &[&str] = &[
    "<command-",
    "<local-command-",
    "<system-reminder>",
    "<user-prompt-submit-hook>",
    "<bash-input>",
    "<bash-stdout>",
    "<bash-stderr>",
    "<environment_context>",
    "<user_instructions>",
    "<permissions instructions>",
    "# AGENTS.md instructions",
    "Caveat: The messages below were generated",
    "[Request interrupted",
];

/// Turns a candidate user message into a title, or `None` when it is not a real prompt.
fn clean_title(text: &str) -> Option<String> {
    let stripped = strip_system_reminders(text);
    let trimmed = stripped.trim();
    if trimmed.is_empty()
        || SKIPPED_PREFIXES
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
    {
        return None;
    }
    let collapsed = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(truncate_chars(&collapsed, MAX_TITLE_CHARS))
}

fn strip_system_reminders(text: &str) -> String {
    const OPEN: &str = "<system-reminder>";
    const CLOSE: &str = "</system-reminder>";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        match rest[start..].find(CLOSE) {
            Some(end) => rest = &rest[start + end + CLOSE.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut short: String = text.chars().take(max.saturating_sub(1)).collect();
    short.push('…');
    short
}

fn fallback_title(harness: &str, session_id: &str) -> String {
    let short: String = session_id.chars().take(8).collect();
    format!("{harness} session {short}")
}

/// Timestamps, working directory and title gathered from one transcript.
#[derive(Default)]
struct Scan {
    cwd: Option<PathBuf>,
    relative: Option<String>,
    first_ms: Option<i64>,
    last_ms: Option<i64>,
    title: Option<String>,
    fallback_title: Option<String>,
    git_branch: Option<String>,
    skipped_lines: usize,
    truncated: bool,
}

impl Scan {
    fn observe(&mut self, timestamp: Option<&str>) {
        if let Some(millis) = timestamp.and_then(parse_rfc3339_millis) {
            self.first_ms = Some(self.first_ms.map_or(millis, |first| first.min(millis)));
            self.last_ms = Some(self.last_ms.map_or(millis, |last| last.max(millis)));
        }
    }

    /// Builds the session when it has a directory and falls within the window.
    fn into_session(
        self,
        harness: ImportHarness,
        session_id: &str,
        files: Vec<SessionFile>,
        since_ms: i64,
    ) -> Option<DiscoveredSession> {
        let (Some(cwd), Some(first), Some(last)) = (self.cwd, self.first_ms, self.last_ms) else {
            return None;
        };
        if last < since_ms {
            return None;
        }
        let title = self
            .title
            .or(self.fallback_title)
            .unwrap_or_else(|| fallback_title(harness_label(harness), session_id));
        Some(DiscoveredSession {
            harness,
            session_id: session_id.to_owned(),
            title,
            cwd,
            cwd_relative: self.relative,
            started_at: format_millis(first),
            ended_at: format_millis(last),
            git_branch: self.git_branch,
            files,
            skipped_lines: self.skipped_lines,
            truncated: self.truncated,
        })
    }
}

fn harness_label(harness: ImportHarness) -> &'static str {
    match harness {
        ImportHarness::Claude => "Claude",
        ImportHarness::Codex => "Codex",
    }
}

fn usable_branch(branch: Option<String>) -> Option<String> {
    branch.filter(|name| !name.is_empty() && name != "HEAD")
}

// ---------------------------------------------------------------------------------------------
// Claude Code

#[derive(Deserialize)]
struct ClaudeMeta {
    #[serde(rename = "type")]
    kind: Option<String>,
    cwd: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "gitBranch")]
    git_branch: Option<String>,
    #[serde(rename = "isMeta", default)]
    is_meta: Option<bool>,
}

#[derive(Deserialize)]
struct ClaudeUserLine {
    message: Option<ClaudeMessage>,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    content: Option<MessageContent>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
}

fn claude_title(line: &[u8]) -> Option<String> {
    let user: ClaudeUserLine = serde_json::from_slice(line).ok()?;
    match user.message?.content? {
        MessageContent::Text(text) => clean_title(&text),
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter(|block| block.kind.as_deref() == Some("text"))
            .filter_map(|block| block.text.as_deref())
            .find_map(clean_title),
    }
}

fn claude_session(
    project_dir: &Path,
    entry: &DirEntry,
    matcher: &RepoMatcher,
    since_ms: i64,
) -> Option<DiscoveredSession> {
    let path = entry.path();
    if !entry.file_type().is_ok_and(|kind| kind.is_file())
        || path.extension().is_none_or(|ext| ext != "jsonl")
    {
        return None;
    }
    let session_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|id| is_safe_session_id(id))?
        .to_owned();
    if modified_before(&path, since_ms) {
        return None;
    }
    let size = file_size(&path)?;
    let scan = scan_claude(&path, matcher).ok()??;
    let mut files = vec![SessionFile {
        name: format!("sessions/claude/{session_id}.jsonl"),
        path: path.clone(),
        size,
    }];
    files.extend(claude_subagents(project_dir, &session_id));
    scan.into_session(ImportHarness::Claude, &session_id, files, since_ms)
}

fn claude_subagents(project_dir: &Path, session_id: &str) -> Vec<SessionFile> {
    let dir = project_dir.join(session_id).join("subagents");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<SessionFile> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let path = entry.path();
            let file_name = path.file_name()?.to_str()?.to_owned();
            if !file_name.ends_with(".jsonl") {
                return None;
            }
            let name = format!("sessions/claude/{session_id}/subagents/{file_name}");
            if !is_safe_file_name(&name) {
                return None;
            }
            let size = file_size(&path)?;
            Some(SessionFile { name, path, size })
        })
        .collect();
    files.sort_by(|a, b| a.name.cmp(&b.name));
    files
}

/// Scans a Claude transcript; `None` when its first working directory is outside the checkouts.
fn scan_claude(path: &Path, matcher: &RepoMatcher) -> io::Result<Option<Scan>> {
    let mut reader = JsonlReader::open(path)?;
    let mut buf = Vec::new();
    let mut scan = Scan::default();
    while let Some(line) = reader.next_line(&mut buf)? {
        if line == Line::TooLong {
            scan.skipped_lines += 1;
            continue;
        }
        if buf.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(meta) = serde_json::from_slice::<ClaudeMeta>(&buf) else {
            scan.skipped_lines += 1;
            continue;
        };
        if scan.cwd.is_none()
            && let Some(cwd) = meta.cwd.as_deref().filter(|cwd| !cwd.is_empty())
        {
            let cwd = PathBuf::from(cwd);
            let Some(relative) = matcher.locate(&cwd) else {
                return Ok(None);
            };
            scan.cwd = Some(cwd);
            scan.relative = relative;
        }
        scan.observe(meta.timestamp.as_deref());
        if scan.git_branch.is_none() {
            scan.git_branch = usable_branch(meta.git_branch);
        }
        if scan.title.is_none()
            && meta.kind.as_deref() == Some("user")
            && meta.is_meta != Some(true)
        {
            scan.title = claude_title(&buf);
        }
    }
    scan.truncated = reader.truncated;
    if scan.truncated {
        for line in tail_lines(path, MAX_LINE_BYTES as u64)? {
            if let Ok(meta) = serde_json::from_slice::<ClaudeMeta>(&line) {
                scan.observe(meta.timestamp.as_deref());
            }
        }
    }
    Ok(Some(scan))
}

// ---------------------------------------------------------------------------------------------
// Codex

#[derive(Deserialize)]
struct CodexFirstLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    payload: Option<CodexSessionMeta>,
}

#[derive(Deserialize)]
struct CodexSessionMeta {
    id: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    git: Option<CodexGit>,
}

#[derive(Deserialize)]
struct CodexGit {
    repository_url: Option<String>,
    branch: Option<String>,
}

#[derive(Deserialize)]
struct CodexLine {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct CodexPayloadLine {
    payload: Option<CodexPayload>,
}

#[derive(Deserialize)]
struct CodexPayload {
    #[serde(rename = "type")]
    kind: Option<String>,
    role: Option<String>,
    message: Option<String>,
    content: Option<Vec<ContentBlock>>,
}

/// Collects `rollout-*.jsonl` files exactly `depth` directory levels below `dir`
/// (`YYYY/MM/DD`), without following symbolic links.
fn collect_rollouts(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if depth > 0 {
            if kind.is_dir() {
                collect_rollouts(&path, depth - 1, out);
            }
        } else if kind.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
        {
            out.push(path);
        }
    }
}

fn codex_session(
    path: &Path,
    matcher: &RepoMatcher,
    remote: Option<&GitHubRemote>,
    since_ms: i64,
) -> Option<DiscoveredSession> {
    if modified_before(path, since_ms) {
        return None;
    }
    let size = file_size(path)?;
    let (session_id, scan) = scan_codex(path, matcher, remote).ok()??;
    let files = vec![SessionFile {
        name: format!("sessions/codex/{session_id}.jsonl"),
        path: path.to_path_buf(),
        size,
    }];
    scan.into_session(ImportHarness::Codex, &session_id, files, since_ms)
}

/// Scans a Codex rollout; `None` when it is not a session or does not belong to the repository.
fn scan_codex(
    path: &Path,
    matcher: &RepoMatcher,
    remote: Option<&GitHubRemote>,
) -> io::Result<Option<(String, Scan)>> {
    let mut reader = JsonlReader::open(path)?;
    let mut buf = Vec::new();
    let Some(Line::Data) = reader.next_line(&mut buf)? else {
        return Ok(None);
    };
    let Ok(first) = serde_json::from_slice::<CodexFirstLine>(&buf) else {
        return Ok(None);
    };
    let (Some("session_meta"), Some(meta)) = (first.kind.as_deref(), first.payload) else {
        return Ok(None);
    };
    let Some(session_id) = meta.id.filter(|id| is_safe_session_id(id)) else {
        return Ok(None);
    };
    let Some(cwd) = meta.cwd.filter(|cwd| !cwd.is_empty()).map(PathBuf::from) else {
        return Ok(None);
    };
    let (repository_url, branch) = meta
        .git
        .map_or((None, None), |git| (git.repository_url, git.branch));
    let relative = match matcher.locate(&cwd) {
        Some(relative) => relative,
        None => {
            let same_repository = remote.is_some_and(|remote| {
                repository_url
                    .as_deref()
                    .and_then(parse_github_remote)
                    .is_some_and(|recorded| recorded.same_repository(remote))
            });
            if !same_repository {
                return Ok(None);
            }
            None
        }
    };
    let mut scan = Scan {
        cwd: Some(cwd),
        relative,
        git_branch: usable_branch(branch),
        ..Scan::default()
    };
    scan.observe(meta.timestamp.as_deref());
    scan.observe(first.timestamp.as_deref());
    while let Some(line) = reader.next_line(&mut buf)? {
        if line == Line::TooLong {
            scan.skipped_lines += 1;
            continue;
        }
        if buf.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(record) = serde_json::from_slice::<CodexLine>(&buf) else {
            scan.skipped_lines += 1;
            continue;
        };
        scan.observe(record.timestamp.as_deref());
        if scan.title.is_none() {
            match record.kind.as_deref() {
                Some("event_msg") => scan.title = codex_event_title(&buf),
                Some("response_item") if scan.fallback_title.is_none() => {
                    scan.fallback_title = codex_response_title(&buf);
                }
                _ => {}
            }
        }
    }
    scan.truncated = reader.truncated;
    if scan.truncated {
        for line in tail_lines(path, MAX_LINE_BYTES as u64)? {
            if let Ok(record) = serde_json::from_slice::<CodexLine>(&line) {
                scan.observe(record.timestamp.as_deref());
            }
        }
    }
    Ok(Some((session_id, scan)))
}

/// Title from an `event_msg` `user_message` record (the user's typed prompt).
fn codex_event_title(line: &[u8]) -> Option<String> {
    let payload = serde_json::from_slice::<CodexPayloadLine>(line)
        .ok()?
        .payload?;
    if payload.kind.as_deref() != Some("user_message") {
        return None;
    }
    clean_title(payload.message.as_deref()?)
}

/// Fallback title from a user `response_item` message (skipping injected context).
fn codex_response_title(line: &[u8]) -> Option<String> {
    let payload = serde_json::from_slice::<CodexPayloadLine>(line)
        .ok()?
        .payload?;
    if payload.kind.as_deref() != Some("message") || payload.role.as_deref() != Some("user") {
        return None;
    }
    payload
        .content?
        .iter()
        .filter(|block| block.kind.as_deref() == Some("input_text"))
        .filter_map(|block| block.text.as_deref())
        .find_map(clean_title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_skip_wrappers_and_collapse_whitespace() {
        assert_eq!(
            clean_title("  Fix the\n\nlogin   bug "),
            Some("Fix the login bug".to_owned())
        );
        assert_eq!(clean_title("<command-name>/clear</command-name>"), None);
        assert_eq!(
            clean_title("<local-command-stdout></local-command-stdout>"),
            None
        );
        assert_eq!(clean_title("[Request interrupted by user]"), None);
        assert_eq!(
            clean_title("<system-reminder>internal</system-reminder>\nReal ask"),
            Some("Real ask".to_owned())
        );
        assert_eq!(clean_title("<system-reminder>unterminated"), None);
        let long = "word ".repeat(40);
        let title = clean_title(&long).unwrap_or_default();
        assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn matcher_prefers_the_deepest_checkout_and_respects_component_boundaries() {
        let matcher = RepoMatcher::new(&[
            PathBuf::from("/nonexistent-yulius/repo"),
            PathBuf::from("/nonexistent-yulius/repo/.claude/worktrees/feat"),
        ]);
        assert_eq!(
            matcher.locate(Path::new("/nonexistent-yulius/repo")),
            Some(None)
        );
        assert_eq!(
            matcher.locate(Path::new("/nonexistent-yulius/repo/crates/app")),
            Some(Some("crates/app".to_owned()))
        );
        assert_eq!(
            matcher.locate(Path::new(
                "/nonexistent-yulius/repo/.claude/worktrees/feat/src"
            )),
            Some(Some("src".to_owned()))
        );
        assert_eq!(
            matcher.locate(Path::new("/nonexistent-yulius/repo-other")),
            None
        );
        assert_eq!(
            matcher.locate(Path::new("/nonexistent-yulius/repo/../elsewhere")),
            None
        );
        assert_eq!(matcher.locate(Path::new("relative/repo")), None);
    }
}
