//! Skill catalog: repository, instance and bundled skills with per-sandbox enablement (SPEC-001
//! SKL-001..005, §7.3, AC-SKL-1..3).
//!
//! Sources, in precedence order when two share a name:
//! 1. repository skills in the sandbox clone (`.agents/skills/*/SKILL.md`, then
//!    `.claude/skills/*/SKILL.md`); the harness CLIs already see them inside the repository, so
//!    they are listed but never installed and cannot be disabled here;
//! 2. instance skills in `<data_dir>/skills/<name>/` (managed by instance administrators);
//! 3. the bundled packs extracted to `<data_dir>/skills/<pack>/<name>/`: Yulius's own skills
//!    (`yulius`), the adapted Open Mercato skills (`open-mercato`) and the adapted superpowers
//!    skills (`superpowers`), see [`pack::PACKS`].
//!
//! Repository content is untrusted (SPEC-002 §8): a skill whose resolved path leaves the
//! repository (for example a symbolic link to a host file) is ignored, reads are bounded, and
//! names must be safe directory names. Disk work runs on blocking threads.

pub mod pack;
pub mod refresh;

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use yulius_contracts::api::skills::{SkillContent, SkillView};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;

/// Directory of instance skills under the data directory.
pub const SKILLS_DIR: &str = "skills";
/// Most skills read from one source directory.
pub const MAX_SKILLS_PER_SOURCE: usize = 200;
/// Largest `SKILL.md` read (content view and frontmatter).
pub const MAX_SKILL_MD_BYTES: u64 = 512 * 1024;
/// Most directory entries examined per skill source directory.
const MAX_DIR_ENTRIES: usize = 10_000;
/// Largest prefix of a `SKILL.md` read to parse its frontmatter.
pub const MAX_FRONTMATTER_BYTES: u64 = 64 * 1024;
/// Longest description kept for listing.
pub const MAX_DESCRIPTION_CHARS: usize = 4_096;
/// Repository directories searched for skills, in precedence order.
pub const REPOSITORY_SKILL_DIRS: [&str; 2] = [".agents/skills", ".claude/skills"];

/// Where a skill comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkillSource {
    /// The sandbox repository.
    Repository,
    /// The instance catalog.
    Instance,
    /// Yulius's own bundled skills.
    Yulius,
    /// The bundled Open Mercato skills, adapted to be universal (MIT).
    OpenMercato,
    /// The bundled skills adapted from obra/superpowers (MIT).
    Superpowers,
}

impl SkillSource {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Repository => "repository",
            Self::Instance => "instance",
            Self::Yulius => "yulius",
            Self::OpenMercato => "open-mercato",
            Self::Superpowers => "superpowers",
        }
    }
}

/// One skill found in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogSkill {
    /// Directory name (the identity used by the API and the harness homes).
    pub name: String,
    /// Frontmatter description (may be empty).
    pub description: String,
    /// Source.
    pub source: SkillSource,
    /// Skill directory.
    pub dir: PathBuf,
    /// Location shown to users (repository-relative or data-directory-relative).
    pub display_path: String,
}

/// `name`/`description` from a `SKILL.md` frontmatter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillMeta {
    /// `name`.
    pub name: Option<String>,
    /// `description`.
    pub description: Option<String>,
}

/// Whether `name` is a safe skill directory name: 1–64 ASCII letters, digits, `.`, `-` or `_`,
/// not starting with `.` (matches the harness installer's rule).
#[must_use]
pub fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// Splits `key: value` for a top-level (unindented) mapping line.
fn top_level_key(line: &str) -> Option<(&str, &str)> {
    if line.starts_with([' ', '\t', '#', '-']) {
        return None;
    }
    let (key, rest) = line.split_once(':')?;
    let key = key.trim_end();
    let valid = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !valid || !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    Some((key, rest.trim()))
}

/// Removes a trailing ` #comment` from a plain scalar.
fn strip_comment(text: &str) -> &str {
    let bytes = text.as_bytes();
    for i in 1..bytes.len() {
        if bytes[i] == b'#' && matches!(bytes[i - 1], b' ' | b'\t') {
            return text[..i].trim_end();
        }
    }
    text
}

/// Parses a quoted scalar starting at `first` and possibly continuing on `rest` lines.
/// Returns the value and how many continuation lines it consumed.
fn quoted(first: &str, rest: &[&str]) -> (String, usize) {
    let quote = first.chars().next().unwrap_or('"');
    let mut text = first[1..].to_owned();
    let mut used = 0;
    loop {
        if let Some(value) = close_quote(&text, quote) {
            return (value, used);
        }
        match rest.get(used) {
            Some(line) => {
                text.push('\n');
                text.push_str(line.trim());
                used += 1;
            }
            None => return (unescape(text.trim_end(), quote), used),
        }
    }
}

/// The unescaped content up to the closing quote, if present.
fn close_quote(text: &str, quote: char) -> Option<String> {
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if quote == '"' && c == '\\' {
            chars.next();
        } else if c == quote {
            if quote == '\'' && chars.peek().is_some_and(|(_, n)| *n == '\'') {
                chars.next();
                continue;
            }
            return Some(unescape(&text[..i], quote));
        }
    }
    None
}

fn unescape(raw: &str, quote: char) -> String {
    // Line breaks inside quoted scalars fold to spaces (blank lines to newlines).
    let folded = fold_lines(&raw.split('\n').map(str::trim).collect::<Vec<_>>());
    if quote == '\'' {
        return folded.replace("''", "'");
    }
    let mut out = String::with_capacity(folded.len());
    let mut chars = folded.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Folds lines: single breaks become spaces, blank lines become newlines.
fn fold_lines(lines: &[&str]) -> String {
    let mut out = String::new();
    let mut pending_break = false;
    for line in lines {
        if line.is_empty() {
            out.push('\n');
            pending_break = false;
            continue;
        }
        if pending_break {
            out.push(' ');
        }
        out.push_str(line);
        pending_break = true;
    }
    out.trim().to_owned()
}

/// Collects a block scalar (`|`/`>` with optional chomping and indentation indicators).
fn block(indicator: &str, rest: &[&str]) -> (String, usize) {
    let mut used = 0;
    let mut lines: Vec<&str> = Vec::new();
    for line in rest {
        if !line.trim().is_empty() && indent_of(line) == 0 {
            break;
        }
        lines.push(line);
        used += 1;
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let common = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| indent_of(l))
        .min()
        .unwrap_or(0);
    let stripped: Vec<&str> = lines
        .iter()
        .map(|l| l.get(common..).unwrap_or(""))
        .collect();
    let text = if indicator.starts_with('>') {
        fold_lines(&stripped.iter().map(|l| l.trim_end()).collect::<Vec<_>>())
    } else {
        stripped.join("\n").trim_end().to_owned()
    };
    (text, used)
}

/// Parses the `name` and `description` of a `SKILL.md` YAML frontmatter.
///
/// Accepts plain, single- and double-quoted scalars (also spanning several lines), block
/// scalars (`|`, `>`, with chomping/indentation indicators) and indented plain continuation
/// lines. Anything that is not a frontmatter yields empty metadata.
#[must_use]
pub fn parse_frontmatter(text: &str) -> SkillMeta {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return SkillMeta::default();
    }
    let body: Vec<&str> = lines
        .take_while(|l| !matches!(l.trim_end(), "---" | "..."))
        .collect();
    let mut meta = SkillMeta::default();
    let mut i = 0;
    while i < body.len() {
        let Some((key, value)) = top_level_key(body[i]) else {
            i += 1;
            continue;
        };
        let rest = &body[i + 1..];
        let (parsed, used) = if value.starts_with(['|', '>']) {
            block(value, rest)
        } else if value.starts_with(['"', '\'']) {
            quoted(value, rest)
        } else {
            let mut parts = vec![strip_comment(value)];
            let mut used = 0;
            for line in rest {
                if line.trim().is_empty() || indent_of(line) == 0 {
                    break;
                }
                parts.push(strip_comment(line.trim()));
                used += 1;
            }
            (fold_lines(&parts), used)
        };
        i += 1 + used;
        let parsed: String = parsed.chars().take(MAX_DESCRIPTION_CHARS).collect();
        match key {
            "name" if meta.name.is_none() => meta.name = Some(parsed),
            "description" if meta.description.is_none() => meta.description = Some(parsed),
            _ => {}
        }
    }
    meta
}

/// Reads at most `limit` bytes of a regular file as lossy UTF-8 (special files such as FIFOs
/// are refused so a hostile repository cannot block the reader; see [`open_regular`]).
fn read_bounded(path: &Path, limit: u64) -> io::Result<String> {
    let mut bytes = Vec::new();
    open_regular(path)?.take(limit).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Opens the regular file `path` for reading. The open itself refuses a link (`O_NOFOLLOW`) and
/// never waits (`O_NONBLOCK`), and the type is checked on the handle: a repository's file an agent
/// swaps for a FIFO after a check by path cannot block the reader (SPEC-507 review round 1).
fn open_regular(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    crate::race::reached(crate::race::Point::SkillFileOpen, path);
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK | nix::libc::O_NOCTTY)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    Ok(file)
}

/// Lists skill directories under `dir` whose `SKILL.md` resolves inside `boundary`.
fn scan_dir(
    dir: &Path,
    boundary: &Path,
    source: SkillSource,
    display_prefix: &str,
    skip: &[&str],
) -> Vec<CatalogSkill> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .take(MAX_DIR_ENTRIES)
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| valid_skill_name(n) && !skip.contains(&n.as_str()))
        .collect();
    names.sort();
    names.truncate(MAX_SKILLS_PER_SOURCE);
    let mut out = Vec::new();
    for name in names {
        let skill_dir = dir.join(&name);
        let Ok(real_dir) = skill_dir.canonicalize() else {
            continue;
        };
        let Ok(real_md) = real_dir.join("SKILL.md").canonicalize() else {
            continue;
        };
        if !real_dir.starts_with(boundary) || !real_md.starts_with(&real_dir) || !real_md.is_file()
        {
            tracing::warn!(skill = %name, "skipping a skill that resolves outside its source");
            continue;
        }
        let meta = read_bounded(&real_md, MAX_FRONTMATTER_BYTES)
            .map(|text| parse_frontmatter(&text))
            .unwrap_or_default();
        out.push(CatalogSkill {
            description: meta.description.unwrap_or_default(),
            source,
            display_path: format!("{display_prefix}/{name}/SKILL.md"),
            dir: real_dir,
            name,
        });
    }
    out
}

/// Scans all sources and resolves name clashes by precedence (repository, instance, pack).
#[must_use]
pub fn scan_catalog(repo: &Path, skills_root: &Path) -> Vec<CatalogSkill> {
    let mut found: Vec<CatalogSkill> = Vec::new();
    if let Ok(repo_root) = repo.canonicalize() {
        for sub in REPOSITORY_SKILL_DIRS {
            found.extend(scan_dir(
                &repo.join(sub),
                &repo_root,
                SkillSource::Repository,
                sub,
                &[],
            ));
        }
    }
    if let Ok(root) = skills_root.canonicalize() {
        let pack_dirs: Vec<&str> = pack::PACKS.iter().map(|p| p.dir).collect();
        found.extend(scan_dir(
            skills_root,
            &root,
            SkillSource::Instance,
            SKILLS_DIR,
            &pack_dirs,
        ));
        for bundled in &pack::PACKS {
            let pack_dir = skills_root.join(bundled.dir);
            if let Ok(pack_root) = pack_dir.canonicalize() {
                found.extend(
                    scan_dir(
                        &pack_dir,
                        &pack_root,
                        bundled.source,
                        &format!("{SKILLS_DIR}/{}", bundled.dir),
                        &[],
                    )
                    .into_iter()
                    .filter(|s| bundled.skills.contains(&s.name.as_str())),
                );
            }
        }
    }
    let mut seen = BTreeSet::new();
    found.retain(|s| seen.insert(s.name.clone()));
    found
}

/// Writes through a temporary file and a rename so readers (skill installs) never see a
/// partial file. No fsync: the extracted packs are a cache that [`extract_packs`] verifies byte
/// for byte against the embedded copy at every startup and rewrites on mismatch, so a write lost
/// in a crash is repaired on the next start.
fn write_atomic(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::other("invalid file name"))?;
    let tmp = target.with_file_name(format!(".{name}.yulius-tmp"));
    fs::File::create(&tmp)?.write_all(bytes)?;
    fs::rename(&tmp, target)
}

/// Removes entries under `dir` that are not part of the pack (bounded walk, never follows
/// symbolic links).
fn prune(
    dir: &Path,
    rel: &str,
    files: &BTreeSet<&str>,
    dirs: &BTreeSet<String>,
    depth: usize,
) -> io::Result<()> {
    if depth > 8 {
        return Ok(());
    }
    for entry in fs::read_dir(dir)?.take(5_000) {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            fs::remove_file(entry.path()).or_else(|_| fs::remove_dir_all(entry.path()))?;
            continue;
        };
        let path_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if dirs.contains(&path_rel) {
                prune(&entry.path(), &path_rel, files, dirs, depth + 1)?;
            } else {
                fs::remove_dir_all(entry.path())?;
            }
        } else if !files.contains(path_rel.as_str()) {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Extracts one embedded pack to `<skills_root>/<pack>/`, rewriting files whose content differs
/// and removing anything else in that directory. Returns the number of files written.
fn extract_pack(skills_root: &Path, bundled: &pack::Pack) -> io::Result<u32> {
    let root = skills_root.join(bundled.dir);
    if fs::symlink_metadata(&root).is_ok_and(|m| !m.is_dir()) {
        fs::remove_file(&root)?;
    }
    fs::create_dir_all(&root)?;
    let mut written = 0;
    let mut files = BTreeSet::new();
    let mut dirs = BTreeSet::new();
    for (rel, bytes) in bundled.files {
        files.insert(*rel);
        let mut parent = Path::new(rel).parent();
        while let Some(p) = parent.filter(|p| !p.as_os_str().is_empty()) {
            dirs.insert(p.to_string_lossy().into_owned());
            parent = p.parent();
        }
        let target = root.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let current = fs::symlink_metadata(&target)
            .ok()
            .filter(fs::Metadata::is_file)
            .and_then(|_| fs::read(&target).ok());
        if current.as_deref() == Some(*bytes) {
            continue;
        }
        if fs::symlink_metadata(&target).is_ok_and(|m| m.is_dir()) {
            fs::remove_dir_all(&target)?;
        }
        write_atomic(&target, bytes)?;
        written += 1;
    }
    prune(&root, "", &files, &dirs, 0)?;
    Ok(written)
}

/// Extracts every embedded pack to `<skills_root>/<pack>/` (see [`pack::PACKS`]). Returns the
/// number of files written (0 when already current).
///
/// # Errors
/// I/O failures.
pub fn extract_packs(skills_root: &Path) -> io::Result<u32> {
    let mut written = 0;
    for bundled in &pack::PACKS {
        written += extract_pack(skills_root, bundled)?;
    }
    Ok(written)
}

async fn blocking<T, F>(f: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(AppError::internal)
}

impl App {
    /// Instance skill directory (`<data_dir>/skills`).
    #[must_use]
    pub fn skills_root(&self) -> PathBuf {
        self.config.data_dir.join(SKILLS_DIR)
    }

    /// Extracts the embedded skill packs (call once at startup; idempotent). Returns the number
    /// of files written.
    ///
    /// # Errors
    /// I/O failures.
    pub async fn install_skill_catalog(&self) -> AppResult<u32> {
        let root = self.skills_root();
        blocking(move || extract_packs(&root))
            .await?
            .map_err(AppError::internal)
    }

    /// The deduplicated catalog of a sandbox.
    ///
    /// # Errors
    /// Blocking-task failures.
    pub async fn skill_catalog(&self, sandbox: &SandboxId) -> AppResult<Vec<CatalogSkill>> {
        let repo = self.sandbox_repo(sandbox);
        let root = self.skills_root();
        blocking(move || scan_catalog(&repo, &root)).await
    }

    async fn skill_states(&self, sandbox: &SandboxId) -> AppResult<HashMap<String, bool>> {
        Ok(self
            .deps
            .store
            .skill_settings(sandbox)
            .await?
            .into_iter()
            .collect())
    }

    fn skill_enabled(skill: &CatalogSkill, states: &HashMap<String, bool>) -> bool {
        skill.source == SkillSource::Repository || states.get(&skill.name).copied().unwrap_or(true)
    }

    /// `GET /api/sandboxes/{sid}/skills`.
    ///
    /// # Errors
    /// `NotFound` without access.
    pub async fn list_skills(&self, ctx: &AuthContext, sandbox: &str) -> AppResult<Vec<SkillView>> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require_for(ctx, &sid, Action::View).await?;
        let states = self.skill_states(&sid).await?;
        Ok(self
            .skill_catalog(&sid)
            .await?
            .iter()
            .map(|s| SkillView {
                name: s.name.clone(),
                description: s.description.clone(),
                source: s.source.as_str().to_owned(),
                path: s.display_path.clone(),
                enabled: Self::skill_enabled(s, &states),
            })
            .collect())
    }

    async fn find_skill(&self, sandbox: &SandboxId, name: &str) -> AppResult<CatalogSkill> {
        if !valid_skill_name(name) {
            return Err(AppError::NotFound);
        }
        self.skill_catalog(sandbox)
            .await?
            .into_iter()
            .find(|s| s.name == name)
            .ok_or(AppError::NotFound)
    }

    /// `GET /api/sandboxes/{sid}/skills/{name}`: the `SKILL.md` text (bounded).
    ///
    /// # Errors
    /// `NotFound` for unknown skills or without access.
    pub async fn skill_content(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        name: &str,
    ) -> AppResult<SkillContent> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require_for(ctx, &sid, Action::View).await?;
        let skill = self.find_skill(&sid, name).await?;
        // Resolved as the catalog resolved it: a link to a file of the skill's own folder is read
        // there (the open follows no link), any other place is not shown.
        let dir = skill.dir.clone();
        let content = blocking(move || {
            let real = dir.join("SKILL.md").canonicalize()?;
            if !real.starts_with(&dir) {
                return Err(io::Error::other("SKILL.md leaves its skill"));
            }
            read_bounded(&real, MAX_SKILL_MD_BYTES)
        })
        .await?
        .map_err(|_| AppError::NotFound)?;
        Ok(SkillContent {
            name: skill.name,
            content,
        })
    }

    /// `PUT /api/sandboxes/{sid}/skills/{name}`: per-sandbox toggle (write access; instance
    /// skills additionally need an instance administrator). Repository skills always stay
    /// available to agents and cannot be toggled.
    ///
    /// # Errors
    /// Access errors, `NotFound`, `Invalid` for repository skills.
    pub async fn set_skill_enabled(
        self: &std::sync::Arc<Self>,
        ctx: &AuthContext,
        sandbox: &str,
        name: &str,
        enabled: bool,
    ) -> AppResult<()> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require_for(ctx, &sid, Action::Configure).await?;
        let skill = self.find_skill(&sid, name).await?;
        match skill.source {
            SkillSource::Repository => {
                return Err(AppError::Invalid(
                    "repository skills are part of the repository and always available to agents; remove or rename them there".into(),
                ));
            }
            SkillSource::Instance => self.require_instance_admin(ctx)?,
            SkillSource::Yulius | SkillSource::OpenMercato | SkillSource::Superpowers => {}
        }
        self.deps
            .store
            .set_skill_enabled(&sid, &skill.name, enabled)
            .await?;
        self.deps
            .store
            .audit(
                self.now(),
                &self.login_of(&ctx.user.id).await,
                if enabled {
                    "skill.enabled"
                } else {
                    "skill.disabled"
                },
                Some(sid.as_str()),
                Some(&skill.name),
            )
            .await?;
        // The sandbox's home follows the switch at once, not at its next run (SPEC-507 SKR-003).
        match self.deps.store.sandbox(&sid).await {
            Ok(Some(record)) => {
                // In a task of its own, which the request waits for: a request dropped meanwhile
                // (the person left) leaves the refresh and its audit to finish (SKR-003, SKR-004).
                let app = std::sync::Arc::clone(self);
                let refresh = tokio::spawn(async move {
                    app.refresh_sandbox_skills(&record, "skill switched").await;
                });
                if let Err(err) = refresh.await {
                    tracing::warn!(error = %err, sandbox = %sid, "skill refresh failed");
                }
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(error = %err, sandbox = %sid, "skill refresh failed"),
        }
        Ok(())
    }

    /// Enabled instance and bundled skills for a run in `sandbox`, as (name, directory)
    /// for `HarnessPort::install_skills`. Repository skills are excluded (the CLIs see them in
    /// the repository), and so are skills shadowed by a repository skill of the same name.
    ///
    /// # Errors
    /// Store failures.
    pub async fn skills_for_run(
        &self,
        sandbox: &SandboxRecord,
    ) -> AppResult<Vec<(String, PathBuf)>> {
        let states = self.skill_states(&sandbox.id).await?;
        Ok(self
            .skill_catalog(&sandbox.id)
            .await?
            .into_iter()
            .filter(|s| s.source != SkillSource::Repository && Self::skill_enabled(s, &states))
            .map(|s| (s.name, s.dir))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn frontmatter_plain_and_continuation() {
        let meta = parse_frontmatter(
            "---\nname: review\ndescription: Review a change\n  carefully and with tests # why\nlicense: MIT\n---\n# Body\n",
        );
        assert_eq!(meta.name.as_deref(), Some("review"));
        assert_eq!(
            meta.description.as_deref(),
            Some("Review a change carefully and with tests")
        );
    }

    #[test]
    fn frontmatter_block_scalars() {
        let folded = parse_frontmatter(
            "---\nname: a\ndescription: >-\n  Folded first line\n  continues here.\n\n  Second paragraph.\nother: x\n---\n",
        );
        assert_eq!(
            folded.description.as_deref(),
            Some("Folded first line continues here.\nSecond paragraph.")
        );
        let literal = parse_frontmatter("---\ndescription: |\n    line one\n    line two\n---\n");
        assert_eq!(literal.description.as_deref(), Some("line one\nline two"));
    }

    #[test]
    fn frontmatter_quoted_scalars() {
        let double = parse_frontmatter(
            "---\nname: \"q\"\ndescription: \"Use when: the user says \\\"go\\\"\n  and more\"\n---\n",
        );
        assert_eq!(double.name.as_deref(), Some("q"));
        assert_eq!(
            double.description.as_deref(),
            Some("Use when: the user says \"go\" and more")
        );
        let single = parse_frontmatter("---\ndescription: 'It''s fine: yes'\n---\n");
        assert_eq!(single.description.as_deref(), Some("It's fine: yes"));
    }

    #[test]
    fn frontmatter_absent_or_malformed() {
        assert_eq!(parse_frontmatter("# no frontmatter"), SkillMeta::default());
        assert_eq!(parse_frontmatter(""), SkillMeta::default());
        let unterminated = parse_frontmatter("---\ndescription: \"open\n");
        assert_eq!(unterminated.description.as_deref(), Some("open"));
        let bom = parse_frontmatter("\u{feff}---\r\nname: x\r\n---\r\n");
        assert_eq!(bom.name.as_deref(), Some("x"));
    }

    /// SPEC-507 review round 1: a `SKILL.md` an agent swaps for a FIFO between the type check
    /// and the open never blocks the reader (the server's start reads every sandbox's repository
    /// skills): the open itself refuses anything but a regular file, at once.
    #[test]
    fn a_fifo_is_refused_by_the_open_itself() -> TestResult {
        use std::os::unix::fs::OpenOptionsExt as _;
        let dir = tempfile::tempdir()?;
        let fifo = dir.path().join("SKILL.md");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU)?;
        let (tx, rx) = std::sync::mpsc::channel();
        let path = fifo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(open_regular(&path).map(|_| ()));
        });
        let opened = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| "opening a FIFO blocked")?;
        assert!(opened.is_err(), "a FIFO is not a regular file");
        // A writer unblocks a reader left behind by a failing build of this test.
        let _ = fs::OpenOptions::new()
            .write(true)
            .custom_flags(nix::libc::O_NONBLOCK)
            .open(&fifo);
        fs::remove_file(&fifo)?;
        fs::write(&fifo, "---\nname: x\n---\n")?;
        assert!(read_bounded(&fifo, 64)?.contains("name: x"));
        Ok(())
    }

    #[test]
    fn names_are_safe() {
        assert!(valid_skill_name("om-fix"));
        assert!(valid_skill_name("a.b_c"));
        assert!(!valid_skill_name(".hidden"));
        assert!(!valid_skill_name("a/b"));
        assert!(!valid_skill_name(""));
        assert!(!valid_skill_name(&"x".repeat(65)));
    }

    fn files_under(dir: &Path) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
        let mut on_disk = BTreeSet::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(current) = stack.pop() {
            for entry in fs::read_dir(&current)? {
                let path = entry?.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let rel = path.strip_prefix(dir)?.to_string_lossy().replace('\\', "/");
                    on_disk.insert(rel);
                }
            }
        }
        Ok(on_disk)
    }

    /// AC-SKL-2: each embedded pack is exactly its catalog directory, which holds exactly the
    /// selected skills plus attribution (and the upstream license of adapted packs); skill names
    /// are unique across packs.
    #[test]
    fn embedded_packs_match_the_catalog() -> TestResult {
        let catalog = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../catalog/skills");
        let mut packs: Vec<String> = fs::read_dir(&catalog)?
            .filter_map(Result::ok)
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        packs.sort();
        let mut listed: Vec<String> = pack::PACKS.iter().map(|p| p.dir.to_owned()).collect();
        listed.sort();
        assert_eq!(packs, listed, "every catalog directory is a pack");
        let mut names = BTreeSet::new();
        for bundled in &pack::PACKS {
            let dir = catalog.join(bundled.dir);
            let embedded: BTreeSet<String> =
                bundled.files.iter().map(|(p, _)| (*p).to_owned()).collect();
            assert_eq!(
                files_under(&dir)?,
                embedded,
                "regenerate skills/pack.rs for {}",
                bundled.dir
            );
            let mut skill_dirs: Vec<String> = fs::read_dir(&dir)?
                .filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            skill_dirs.sort();
            let mut selected: Vec<String> =
                bundled.skills.iter().map(|s| (*s).to_owned()).collect();
            selected.sort();
            assert_eq!(skill_dirs, selected, "{}", bundled.dir);
            for skill in bundled.skills {
                assert!(names.insert(*skill), "{skill} is in two packs");
                let text = fs::read_to_string(dir.join(skill).join("SKILL.md"))?;
                let meta = parse_frontmatter(&text);
                assert_eq!(meta.name.as_deref(), Some(*skill));
                assert!(meta.description.is_some_and(|d| d.len() > 40), "{skill}");
            }
            assert!(embedded.contains("README.md"), "{}", bundled.dir);
            let readme = fs::read_to_string(dir.join("README.md"))?;
            if let Some((repository, commit)) = bundled.upstream {
                assert!(embedded.contains("LICENSE"), "{}", bundled.dir);
                assert!(readme.contains(repository) && readme.contains(commit));
                assert!(fs::read_to_string(dir.join("LICENSE"))?.starts_with("MIT License"));
            }
        }
        assert!(
            !names.contains("om-verify-in-repo"),
            "removed (F 2026-10-10)"
        );
        Ok(())
    }

    /// The adapted skills are universal and autonomous (F 2026-10-10): each shows where it comes
    /// from, and none depends on Open Mercato's own harness or waits for a person.
    #[test]
    fn adapted_skills_are_universal_autonomous_and_attributed() -> TestResult {
        let catalog = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../catalog/skills");
        let forbidden = [
            "agentic.config",
            ".ai/skills",
            "om-auto-",
            "om-setup-",
            "om-discover",
            "om-verify-in-repo",
            "tracker descriptor",
            "qa-approved",
            "needs-qa",
            "product-brief",
            "--autonomous",
            "interactive",
            "manual mode",
            "human partner",
            "hard stop",
            "ask the user",
            "wait for the user",
        ];
        for bundled in &pack::PACKS {
            for (rel, bytes) in bundled.files {
                if !rel.ends_with(".md") || *rel == "README.md" {
                    continue;
                }
                let text = String::from_utf8_lossy(bytes).to_lowercase();
                for word in forbidden {
                    assert!(
                        !text.contains(word),
                        "{}/{rel} mentions {word:?}",
                        bundled.dir
                    );
                }
            }
            for skill in bundled.skills {
                let text =
                    fs::read_to_string(catalog.join(bundled.dir).join(skill).join("SKILL.md"))?;
                let note = match bundled.source {
                    SkillSource::OpenMercato => Some(
                        "> Adapted from Open Mercato's skills (MIT), rephrased to be universal; thanks to the Open Mercato team for this contribution.",
                    ),
                    SkillSource::Superpowers => Some("> Adapted from obra/superpowers (MIT"),
                    _ => None,
                };
                if let Some(note) = note {
                    assert!(text.contains(note), "{skill} lacks its attribution note");
                }
            }
        }
        Ok(())
    }

    /// SPEC-484 PLN-007, PLN-008: the plan rule reads the same wherever agents are told it. The
    /// plan holds all the known work; the numbers are the constants' (task #465 read "3-12 steps"
    /// in one text and "3-10 slices" in another, and kept its 18 slices out of its plan).
    #[test]
    fn the_plan_rule_reads_the_same_everywhere() -> TestResult {
        use yulius_domain::spec_format::{TemplateKind, scaffold_files, spec_template};
        use yulius_domain::steps::{COARSE_BELOW, MAX_STEPS, PER_PULL_REQUEST};
        let per = format!("{COARSE_BELOW}-{PER_PULL_REQUEST}");
        let most = format!("at most {MAX_STEPS}");
        let flat = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut texts: Vec<(String, String)> = Vec::new();
        for rel in [
            "catalog/skills/yulius/delivery-pipeline/SKILL.md",
            "catalog/skills/open-mercato/om-spec-writing/SKILL.md",
            "catalog/workflows/feature.md",
        ] {
            texts.push((rel.to_owned(), flat(&fs::read_to_string(root.join(rel))?)));
        }
        let scaffold = scaffold_files("Online Shop", TemplateKind::WebApplication);
        let agents = scaffold
            .iter()
            .find(|(path, _)| path == "AGENTS.md")
            .ok_or("the scaffold has an AGENTS.md")?;
        texts.push(("the scaffolded AGENTS.md".to_owned(), flat(&agents.1)));
        let tools = crate::mcp::tool_descriptors();
        let tool = |name: &str| -> Result<String, String> {
            tools
                .iter()
                .find(|t| t.name == name)
                .map(|t| flat(&t.description))
                .ok_or_else(|| format!("no tool {name}"))
        };
        texts.push(("the steps_set tool".to_owned(), tool("steps_set")?));
        for (place, text) in &texts {
            assert!(
                text.contains("all the work"),
                "{place} does not say the plan holds all the work known"
            );
            assert!(text.contains(&per), "{place} does not say {per}");
            assert!(
                text.contains(&most) || text.contains(&format!("1-{MAX_STEPS}")),
                "{place} does not say {most}"
            );
            assert!(text.contains("found later"), "{place}: a step added later");
            for gone in ["3-10", "1-30", "at most 30"] {
                assert!(!text.contains(gone), "{place} still says {gone:?}");
            }
        }
        // The merge delivers the finished part: nobody is told that every step has to be done.
        let pipeline = &texts[0].1;
        assert!(
            pipeline.contains("delivers the finished part")
                && !pipeline.contains("every step is done"),
            "delivery-pipeline, merge on green"
        );
        let merge = tool("delivery_merge")?;
        assert!(
            merge.contains("delivers the finished part of your plan")
                && merge.contains(
                    "a step of the plan is finished (and none is in progress when the call has a \
                     new head to deliver)"
                )
                && merge.contains("a playground squashes a task once, when every step is finished")
                && merge.contains("a commit on the branch drops an armed merge")
                && !merge.contains("every step is done"),
            "{merge}"
        );
        assert!(tool("spec_check")?.contains(&most), "spec_check");
        assert!(
            tool("step_update")?.contains("Before you finish the last open step"),
            "step_update"
        );
        // A new spec's Plan asks for it where the agent writes the checklist.
        let spec = spec_template(
            "A task",
            &yulius_domain::ids::TaskId::parse("t1").map_err(|e| e.to_string())?,
            yulius_domain::cron::CivilDate::new(2026, 11, 13).ok_or("date")?,
        );
        assert!(
            flat(&spec)
                .contains("of all the work known now, later slices and pull requests included"),
            "{spec}"
        );
        Ok(())
    }

    /// SPEC-174 (task #164, ADR-0141): every place that states the review rule says the same
    /// three things — a finding in the change's own code is fixed in its pull request whatever the
    /// round, only pre-existing bugs, unrelated flaky tests and leftover nits become issues, and
    /// deferring an own-code finding is "request changes" — and none still allows the deferral.
    #[test]
    fn review_findings_in_own_code_are_never_deferred() -> TestResult {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for rel in [
            "catalog/skills/yulius/delivery-pipeline/SKILL.md",
            "catalog/skills/open-mercato/om-code-review/SKILL.md",
            "catalog/skills/open-mercato/om-code-review/references/review-checklist.md",
            "catalog/workflows/feature.md",
            "catalog/workflows/fix.md",
            ".apm/skills/yulius-review/SKILL.md",
            ".apm/skills/yulius-verify/SKILL.md",
            ".apm/skills/yulius-git/SKILL.md",
            "docs/engineering/workflow.md",
        ] {
            let text = fs::read_to_string(root.join(rel))?
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            for stated in [
                "own code",
                "after 3 rounds",
                "pre-existing",
                "flaky test",
                "the change did not cause",
                "outside the change",
                "request changes",
            ] {
                assert!(text.contains(stated), "{rel} does not say {stated:?}");
            }
            assert!(
                text.contains("re-review until approval under the task's existing authorization")
                    || text.contains("re-review until approval without asking the user"),
                "{rel} does not authorize independent re-review through approval"
            );
            assert!(
                [
                    "never becomes a follow-up issue",
                    "never as a follow-up issue"
                ]
                .iter()
                .any(|never| text.contains(never)),
                "{rel} does not say an own-code finding never becomes a follow-up issue"
            );
            for gone in [
                "follow-up issue accepts",
                "reason and follow-up issue",
                "decide, record the reasoning",
                "the author decides on the rest",
                "follow-ups, bugs you find",
                "at most 3 rounds.",
                "up to 3 rounds",
                "split the change or ask a person",
                "splits the change or asks a person",
                "split the change, or stop and ask a person",
                "splits the change, or stops and asks a person",
            ] {
                assert!(!text.contains(gone), "{rel} still says {gone:?}");
            }
        }
        // The verdict covers a major in older code too, and a pre-existing blocker still blocks.
        for rel in [
            "catalog/skills/open-mercato/om-code-review/SKILL.md",
            "catalog/skills/open-mercato/om-code-review/references/review-checklist.md",
        ] {
            let text = fs::read_to_string(root.join(rel))?
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            for stated in [
                "pre-existing majors filed as issues with the evidence",
                "pre-existing or not",
            ] {
                assert!(text.contains(stated), "{rel} does not say {stated:?}");
            }
        }
        Ok(())
    }

    /// Shared prompts defer to the active harness's policy in the task briefing (SPEC-524).
    #[test]
    fn shared_review_instructions_follow_the_task_briefing() -> TestResult {
        const RULE: &str =
            "follow the task briefing for the reviewer and model of the active harness.";
        let normal = |text: &str| {
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for rel in [
            "catalog/skills/yulius/delivery-pipeline/SKILL.md",
            "catalog/skills/open-mercato/om-code-review/SKILL.md",
            ".apm/skills/yulius-review/SKILL.md",
            ".apm/skills/yulius-git/SKILL.md",
            "docs/engineering/workflow.md",
            "docs/engineering/git.md",
            "AGENTS.md",
        ] {
            let text = normal(&fs::read_to_string(root.join(rel))?);
            assert!(
                text.contains(&normal(RULE)),
                "{rel} does not state the rule"
            );
        }
        let review = normal(&fs::read_to_string(
            root.join("catalog/skills/open-mercato/om-code-review/SKILL.md"),
        )?);
        assert!(review.contains("report that ci awaits delivery and give the code verdict"));
        assert!(!review.contains("never approve without reading the head's yulius ci run"));
        for rel in [
            "catalog/workflows/feature.md",
            "catalog/workflows/fix.md",
            "crates/yulius-domain/src/request.rs",
            "crates/yulius-app/src/mcp.rs",
        ] {
            let text = normal(&fs::read_to_string(root.join(rel))?);
            assert!(text.contains("reviewer from the task briefing"), "{rel}");
            assert!(!text.contains("`yulius-reviewer`"), "{rel}");
        }
        Ok(())
    }

    /// SPEC-493 CIR-004 (AC-12): one rule for local checks, stated the same way wherever checks
    /// are described; the short form where a budget applies; and none of the old wordings that
    /// asked for the whole gate before a handoff or a review.
    #[test]
    fn every_place_states_one_rule_for_local_checks() -> TestResult {
        const RULE: &str = "while iterating, run the checks your change touches. the whole gate \
            is yulius ci's job: it tests the head while the reviewer reads. do not run the whole \
            gate locally before a review or between rounds, except to reproduce a ci failure or \
            when ci cannot run (a playground without ci, an outage).";
        const SHORT: &str = "the whole gate is yulius ci's";
        const OLD: [&str; 8] = [
            "`scripts/dev ci` before the review",
            "use the full gate after implementation stabilizes",
            "then run every gate of the project, in order, until green",
            "`scripts/dev ci` before handoff",
            "run every gate of the project before anything leaves your machine",
            "never approve without running the full gate",
            "6. finish with evidence: run every project gate",
            "for rust/tooling changes run `scripts/dev ci`",
        ];
        let normal = |text: &str| {
            text.replace("\\\n", " ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let read = |rel: &str| -> Result<String, Box<dyn std::error::Error>> {
            Ok(normal(&fs::read_to_string(root.join(rel))?))
        };
        for rel in [
            "docs/engineering/workflow.md",
            ".apm/skills/yulius-verify/SKILL.md",
            "AGENTS.md",
            "catalog/skills/open-mercato/om-check-and-commit/SKILL.md",
            "catalog/skills/open-mercato/om-code-review/SKILL.md",
            "catalog/skills/yulius/delivery-pipeline/SKILL.md",
            "catalog/skills/open-mercato/om-fix/SKILL.md",
            "docs/engineering/git.md",
            "docs/engineering/development.md",
        ] {
            let text = read(rel)?;
            assert!(
                text.contains(&normal(RULE)),
                "{rel} does not state the rule"
            );
        }
        for rel in [
            "catalog/workflows/feature.md",
            "catalog/workflows/fix.md",
            "catalog/workflows/chore.md",
            "crates/yulius-domain/src/briefing.rs",
        ] {
            assert!(
                read(rel)?.contains(SHORT),
                "{rel} does not say the short form"
            );
        }
        // The reviewer reads the head's run and runs the tests it doubts.
        let review = read("catalog/skills/open-mercato/om-code-review/SKILL.md")?;
        assert!(
            review.contains("read the yulius ci run of the head you review"),
            "om-code-review does not send the reviewer to the head's run"
        );
        for rel in [
            "docs/engineering/workflow.md",
            ".apm/skills/yulius-verify/SKILL.md",
            ".apm/skills/yulius-rust-slice/SKILL.md",
            ".apm/skills/yulius-runner/SKILL.md",
            "AGENTS.md",
            "catalog/skills/open-mercato/om-check-and-commit/SKILL.md",
            "catalog/skills/open-mercato/om-code-review/SKILL.md",
            "catalog/skills/yulius/delivery-pipeline/SKILL.md",
            "catalog/skills/open-mercato/om-fix/SKILL.md",
            "docs/engineering/git.md",
            "docs/engineering/development.md",
            "crates/yulius-domain/src/briefing.rs",
        ] {
            let text = read(rel)?;
            for old in OLD {
                assert!(!text.contains(old), "{rel} keeps the old rule: {old}");
            }
        }
        Ok(())
    }

    /// SPEC-493 APR-005 (AC-12): the rule for a follow-up after an approval, stated the same way
    /// wherever reviews and merges are described.
    #[test]
    fn every_place_states_the_follow_up_bound() -> TestResult {
        const RULE: &str = "after an approval, fix the remaining nits within 20 product lines and \
            merge without another round: `delivery_merge` with the approved head counts them \
            (tests and documents do not count). more than that, or a file leaving the change, \
            needs a new round.";
        let normal = |text: &str| {
            text.replace("\\\n", " ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for rel in [
            "catalog/skills/yulius/delivery-pipeline/SKILL.md",
            "catalog/skills/open-mercato/om-code-review/SKILL.md",
            "catalog/skills/yulius/git-hygiene/SKILL.md",
            ".apm/skills/yulius-review/SKILL.md",
            ".apm/skills/yulius-git/SKILL.md",
            "docs/engineering/workflow.md",
            "docs/engineering/git.md",
            "AGENTS.md",
        ] {
            let text = normal(&fs::read_to_string(root.join(rel))?);
            assert!(
                text.contains(&normal(RULE)),
                "{rel} does not state the rule"
            );
        }
        let tool = normal(&fs::read_to_string(
            root.join("crates/yulius-app/src/mcp.rs"),
        )?);
        assert!(
            tool.contains("across at most 20 product lines after it"),
            "delivery_merge does not say the bound"
        );
        Ok(())
    }

    /// SPEC-493 RLM-004 (AC-12): resume a cut-off reviewer, never start a new one for the same
    /// round; a reviewer runs at high effort, never max: the same everywhere reviews are
    /// described.
    #[test]
    fn every_place_says_resume_a_cut_off_reviewer() -> TestResult {
        const RULE: &str = "a usage limit can cut a reviewer off before its report: resume that \
            same reviewer by its agent id, never start a new one for the same \
            round. a reviewer runs at high effort, never max.";
        let normal = |text: &str| {
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for rel in [
            "catalog/skills/yulius/delivery-pipeline/SKILL.md",
            "catalog/skills/open-mercato/om-code-review/SKILL.md",
            ".apm/skills/yulius-review/SKILL.md",
            ".apm/skills/yulius-git/SKILL.md",
            "docs/engineering/workflow.md",
            "docs/engineering/git.md",
            "AGENTS.md",
        ] {
            let text = normal(&fs::read_to_string(root.join(rel))?);
            assert!(
                text.contains(&normal(RULE)),
                "{rel} does not state the rule"
            );
        }
        // The reviewer agents Yulius defines run at that effort.
        assert_eq!(yulius_domain::review::REVIEWER_EFFORT, "high");
        Ok(())
    }

    /// SPEC-493 MUT-002 (AC-12), SPEC-506 FST-004: mutation testing beside the review, the same
    /// everywhere tests and reviews are described; this repository names its command; the workflows send
    /// the agent to `delivery-pipeline`, which states it.
    #[test]
    fn every_place_says_mutation_testing_runs_beside_the_review() -> TestResult {
        const RULE: &str = "mutation testing runs beside the review: when you ask for a pull \
            request's first review, start the project's mutation command in the background and let \
            the reviewer read meanwhile; when it ends, put its summary line in the spec's evidence \
            and fix each missed mutant test-first before the merge. after a later round's fix, run \
            it on that fix's lines alone. a project without one undoes by hand the few lines its \
            new tests are meant to hold and says what failed. the reviewer may undo a single line \
            it doubts and runs no campaign of its own. nobody writes a mutation script by hand. a \
            test that cannot fail is still fixed in the same pull request (adr-0141).";
        const HERE: &str = "here the command is `scripts/dev mutants`.";
        let normal = |text: &str| {
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let read = |rel: &str| -> Result<String, Box<dyn std::error::Error>> {
            Ok(normal(&fs::read_to_string(root.join(rel))?))
        };
        for rel in [
            "AGENTS.md",
            "docs/engineering/development.md",
            ".apm/skills/yulius-review/SKILL.md",
            ".apm/skills/yulius-verify/SKILL.md",
            "catalog/skills/open-mercato/om-code-review/SKILL.md",
            "catalog/skills/superpowers/test-driven-development/SKILL.md",
            "catalog/skills/yulius/delivery-pipeline/SKILL.md",
            "catalog/skills/yulius/infra-bootstrap/SKILL.md",
        ] {
            let guide = read(rel)?;
            for requirement in [
                "1–8 listed changed-line mutants",
                "one worker and 10 minutes",
                "16 listed changed-line mutants",
                "two workers and 30 minutes",
                "run-owned disk scratch",
            ] {
                assert!(guide.contains(requirement), "{rel} lacks {requirement}");
            }
        }
        for rel in [
            "catalog/skills/open-mercato/om-code-review/SKILL.md",
            "catalog/skills/yulius/delivery-pipeline/SKILL.md",
            "catalog/skills/superpowers/test-driven-development/SKILL.md",
        ] {
            assert!(
                read(rel)?.contains(&normal(RULE)),
                "{rel} does not state the rule"
            );
        }
        for rel in [
            ".apm/skills/yulius-verify/SKILL.md",
            ".apm/skills/yulius-review/SKILL.md",
            "docs/engineering/development.md",
        ] {
            let text = read(rel)?;
            assert!(
                text.contains(&normal(&format!("{RULE} {HERE}"))),
                "{rel} does not state the rule with this repository's command"
            );
        }
        for rel in [
            "catalog/skills/open-mercato/om-code-review/SKILL.md",
            "catalog/skills/yulius/delivery-pipeline/SKILL.md",
            "catalog/skills/superpowers/test-driven-development/SKILL.md",
            ".apm/skills/yulius-verify/SKILL.md",
            ".apm/skills/yulius-review/SKILL.md",
            "docs/engineering/development.md",
            "AGENTS.md",
        ] {
            assert!(
                !read(rel)?.contains("on the first head you ask a review for"),
                "{rel} keeps the rule that waits for the run before the review"
            );
        }
        // The workflows' instructions have no byte to spare (1,444 with their heading, #555):
        // they reach the rule through `delivery-pipeline`, which each tells the agent to follow.
        for rel in ["catalog/workflows/feature.md", "catalog/workflows/fix.md"] {
            assert!(
                read(rel)?.contains("follow `delivery-pipeline`"),
                "{rel} does not send the agent to the rule"
            );
        }
        Ok(())
    }

    /// SPEC-493 SCO-003 (AC-12): what to do on an overlapping claim, the same where claims are
    /// described; the briefing, which has a budget, says its short form.
    #[test]
    fn every_place_says_what_to_do_on_an_overlap() -> TestResult {
        const RULE: &str = "on an overlap both tasks are told: wait for the other's pull request \
            to merge and merge the integration branch before editing those files, or agree who \
            changes what by a note on the claim.";
        let normal = |text: &str| {
            text.replace("\\\n", " ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for rel in [
            "catalog/skills/yulius/delivery-pipeline/SKILL.md",
            "crates/yulius-app/src/mcp.rs",
        ] {
            let text = normal(&fs::read_to_string(root.join(rel))?);
            assert!(
                text.contains(&normal(RULE)),
                "{rel} does not state the rule"
            );
        }
        let briefing = normal(&fs::read_to_string(
            root.join("crates/yulius-domain/src/briefing.rs"),
        )?);
        assert!(
            briefing.contains("on an overlap, wait for the other's pr or agree by claim note"),
            "the briefing does not say the short form"
        );
        // None keeps the old rule (review round 1 of fix 9).
        for rel in [
            "crates/yulius-app/src/mcp.rs",
            "crates/yulius-app/src/scopes.rs",
            "crates/yulius-domain/src/briefing.rs",
            "catalog/skills/yulius/delivery-pipeline/SKILL.md",
            ".ai/specs/SPEC-002-yulius-architecture.md",
        ] {
            let text = normal(&fs::read_to_string(root.join(rel))?);
            for old in [
                "nobody else is told",
                "overlaps are shown to you",
                "to the claiming agent only",
                "nobody else is messaged",
            ] {
                assert!(!text.contains(old), "{rel} keeps the old rule: {old}");
            }
        }
        Ok(())
    }

    /// Hostile repositories: special files and links leaving the repository are ignored.
    #[test]
    fn hostile_repository_files_are_ignored() -> TestResult {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("repo");
        let skill = repo.join(".agents/skills/pipe");
        fs::create_dir_all(&skill)?;
        nix::unistd::mkfifo(&skill.join("SKILL.md"), nix::sys::stat::Mode::S_IRWXU)?;
        let outside = temp.path().join("outside.toml");
        fs::write(&outside, "[commands]\ntest = \"curl evil\"\n")?;
        std::os::unix::fs::symlink(&outside, repo.join("yulius.toml"))?;
        let catalog = scan_catalog(&repo, &temp.path().join("skills"));
        assert!(catalog.is_empty(), "{catalog:?}");
        Ok(())
    }

    #[test]
    fn extraction_is_idempotent_and_restores_edits() -> TestResult {
        let temp = tempfile::tempdir()?;
        let dir = temp.path().to_path_buf();
        let total: usize = pack::PACKS.iter().map(|p| p.files.len()).sum();
        assert_eq!(extract_packs(&dir)? as usize, total);
        assert_eq!(extract_packs(&dir)?, 0);
        let skill_md = dir.join("open-mercato/om-fix/SKILL.md");
        fs::write(&skill_md, "local edit")?;
        fs::write(dir.join("open-mercato/om-fix/stray.md"), "x")?;
        fs::create_dir_all(dir.join("open-mercato/om-verify-in-repo"))?;
        fs::create_dir_all(dir.join("yulius/old-skill"))?;
        assert_eq!(extract_packs(&dir)?, 1);
        assert!(fs::read_to_string(&skill_md)?.contains("name: om-fix"));
        assert!(!dir.join("open-mercato/om-fix/stray.md").exists());
        assert!(!dir.join("open-mercato/om-verify-in-repo").exists());
        assert!(!dir.join("yulius/old-skill").exists());
        assert!(dir.join("yulius/git-hygiene/SKILL.md").is_file());
        assert!(
            dir.join("superpowers/test-driven-development/SKILL.md")
                .is_file()
        );
        let catalog = scan_catalog(&temp.path().join("no-repo"), &dir);
        let sources: BTreeSet<&str> = catalog.iter().map(|s| s.source.as_str()).collect();
        assert_eq!(
            sources,
            BTreeSet::from(["open-mercato", "superpowers", "yulius"]),
            "bundled packs are never listed as instance skills"
        );
        assert!(
            catalog
                .iter()
                .all(|s| !matches!(s.source, SkillSource::Repository | SkillSource::Instance))
        );
        Ok(())
    }

    /// SPEC-506 FST-001 (AC-1): no message Yulius types after a red run, a failed merge
    /// candidate, a dropped armed merge or failed checks, nor on a green head with no merge
    /// armed, sends an approved author to a new round: each says the two cases
    /// ([`yulius_domain::review::KEEP_APPROVAL`]: an approval carries across 20 product lines;
    /// with none yet the review comes first). Only a conflict's messages ask for the review of
    /// the new head alone, since a merge that resolved a conflict needs one (ADR-0345).
    #[test]
    fn no_typed_message_asks_for_a_round_the_count_makes_needless() -> TestResult {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let keep = yulius_domain::review::KEEP_APPROVAL;
        assert!(
            keep.contains("`delivery_merge` with that approved head")
                && keep.contains("at most 20 product lines in all since it")
                && keep.contains("with no approval yet, a review of the head comes first"),
            "the sentence names the call, the bound and the case with no approval: {keep}"
        );
        for (rel, uses) in [
            ("crates/yulius-app/src/pr_watch.rs", 4),
            ("crates/yulius-app/src/merge_train.rs", 2),
            ("crates/yulius-app/src/agent_wait.rs", 2),
            ("crates/yulius/src/wait_cli.rs", 2),
        ] {
            let text = fs::read_to_string(root.join(rel))?;
            let source = match text.find("#[cfg(test)]\nmod tests") {
                Some(at) => &text[..at],
                None => text.as_str(),
            };
            // Each message is a string literal; a line continuation joins its lines. Every
            // segment between quotes is read, so no escaped quote can hide one (review round 1).
            let joined = source.replace("\\\n", " ");
            let normal: String = joined.split_whitespace().collect::<Vec<_>>().join(" ");
            for literal in normal.split('"') {
                if literal.contains("review of the new head")
                    || literal.contains("its review approved it")
                    || literal.contains("once the independent review approved")
                    || literal.contains("independent review of that head")
                {
                    assert!(
                        literal.contains("conflict"),
                        "{rel} sends the author to a new round the count makes needless: \
                         {literal}"
                    );
                }
            }
            assert!(
                source.matches("KEEP_APPROVAL").count() >= uses,
                "{rel} does not name delivery_merge with the approved head {uses} time(s)"
            );
        }
        Ok(())
    }

    /// SPEC-506 FST-002 (AC-2): a reviewer does not ask for changes because of CI alone, the
    /// same everywhere the review is described, and the reviewer agents' prompt leaves it to
    /// `om-code-review` (SPEC-507 SKR-006); the old
    /// wording that made any failed job a blocker is gone.
    #[test]
    fn every_place_says_ci_alone_is_no_request_for_changes() -> TestResult {
        const RULE: &str = "a reviewer does not ask for changes because of ci alone: \
            `delivery_merge` merges only a head whose own yulius ci run passed. when the review \
            finds nothing to fix and the head's run is still going, or failed in a way the change \
            does not cause, the verdict is approve and the report says what ci still has to do. a \
            failure the change causes (its own test, format or lint, or a contract its files \
            break) is a blocker.";
        const OLD: [&str; 3] = [
            "a failed job is a blocker, even if it looks unrelated",
            "a failing gate included",
            "any unexecuted configured command keeps the verdict at request changes until \
             validation runs.",
        ];
        let normal = |text: &str| {
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for rel in [
            "catalog/skills/open-mercato/om-code-review/SKILL.md",
            "catalog/skills/open-mercato/om-code-review/references/review-checklist.md",
            "catalog/skills/yulius/delivery-pipeline/SKILL.md",
            ".apm/skills/yulius-review/SKILL.md",
            "docs/engineering/workflow.md",
        ] {
            let text = normal(&fs::read_to_string(root.join(rel))?);
            assert!(
                text.contains(&normal(RULE)),
                "{rel} does not state the rule"
            );
        }
        for rel in [
            "catalog/skills/open-mercato/om-code-review/SKILL.md",
            "catalog/skills/open-mercato/om-code-review/references/review-checklist.md",
            "catalog/skills/open-mercato/om-code-review/references/output-format.md",
        ] {
            let text = normal(&fs::read_to_string(root.join(rel))?);
            for old in OLD {
                assert!(!text.contains(&normal(old)), "{rel} keeps {old:?}");
            }
        }
        // The reviewer agents' prompt, which a session keeps across an update, sends the
        // reviewer to `om-code-review` for the rule instead of saying it (SPEC-507 SKR-006).
        let prompt = normal(yulius_domain::review::REVIEWER_PROMPT);
        assert!(
            prompt.contains("load the skill `om-code-review`") && !prompt.contains(" ci "),
            "the reviewer agents' prompt does not leave the rule to the skill"
        );
        Ok(())
    }

    /// SPEC-506 FST-003 (AC-2): the author proves each fix before asking for another round, the
    /// same everywhere fixes and reviews are described; this repository names its command for a
    /// fix's lines.
    #[test]
    fn every_place_says_prove_each_fix_before_another_round() -> TestResult {
        const RULE: &str = "before asking for another round, answer each finding of the last \
            report with the test that fails without its fix (or why no test can), check every \
            finding of that report against the new head, and run the project's mutation command \
            on the lines of the fix where it can take them.";
        const HERE: &str = "here: `scripts/dev mutants --base <reviewed head>`.";
        let normal = |text: &str| {
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for rel in [
            "catalog/skills/open-mercato/om-fix/SKILL.md",
            "catalog/skills/open-mercato/om-code-review/SKILL.md",
            "catalog/skills/yulius/delivery-pipeline/SKILL.md",
        ] {
            let text = normal(&fs::read_to_string(root.join(rel))?);
            assert!(
                text.contains(&normal(RULE)),
                "{rel} does not state the rule"
            );
        }
        for rel in [
            ".apm/skills/yulius-review/SKILL.md",
            "docs/engineering/workflow.md",
        ] {
            let text = normal(&fs::read_to_string(root.join(rel))?);
            assert!(
                text.contains(&normal(&format!("{RULE} {HERE}"))),
                "{rel} does not state the rule with this repository's command"
            );
        }
        Ok(())
    }

    /// SPEC-506 FST-006 (AC-4): the quick checks before each published commit, the same in
    /// `om-check-and-commit` (every project) and AGENTS.md (with this repository's command).
    #[test]
    fn every_place_says_run_the_quick_checks_before_a_published_commit() -> TestResult {
        const RULE: &str = "before each commit you publish, run the project's quick checks: its \
            format check and the tests that read the whole repository (contracts, generated \
            lists, palettes), which a change touches whenever it changes a file they read. they \
            take seconds, and each one that fails in ci costs a whole run.";
        let normal = |text: &str| {
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for rel in [
            "catalog/skills/open-mercato/om-check-and-commit/SKILL.md",
            "AGENTS.md",
        ] {
            let text = normal(&fs::read_to_string(root.join(rel))?);
            assert!(
                text.contains(&normal(RULE)),
                "{rel} does not state the rule"
            );
        }
        let agents = normal(&fs::read_to_string(root.join("AGENTS.md"))?);
        assert!(
            agents.contains("here: `scripts/dev quick`"),
            "AGENTS.md does not name this repository's command"
        );
        Ok(())
    }
}
