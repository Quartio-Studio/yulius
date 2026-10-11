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

    /// Whether skills of this source ship with Yulius (read-only, per-sandbox enablement).
    #[must_use]
    pub fn is_bundled(self) -> bool {
        !matches!(self, Self::Repository | Self::Instance)
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
/// are refused so a hostile repository cannot block the reader).
fn read_bounded(path: &Path, limit: u64) -> io::Result<String> {
    if !fs::metadata(path)?.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(limit).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
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
        self.require(&ctx.user, &sid, Action::View).await?;
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
        self.require(&ctx.user, &sid, Action::View).await?;
        let skill = self.find_skill(&sid, name).await?;
        let path = skill.dir.join("SKILL.md");
        let content = blocking(move || read_bounded(&path, MAX_SKILL_MD_BYTES))
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
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        name: &str,
        enabled: bool,
    ) -> AppResult<()> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::Configure).await?;
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
            .await
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
        assert!(catalog.iter().all(|s| s.source.is_bundled()));
        Ok(())
    }
}
