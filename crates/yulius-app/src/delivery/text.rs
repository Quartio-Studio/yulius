//! Pull request titles and bodies, clipping, and reading and finding specs.

use super::*;

impl App {
    /// Reads and validates the task spec; a task that needs none (DSC-013: research, a
    /// question, docs or a chore by its workflow, or a docs-only change without one) counts as
    /// valid without it.
    pub(super) async fn dlv_check_spec(&self, task: &TaskRecord) -> SpecCheck {
        let mut check = self.dlv_read_spec(task).await;
        // A valid spec settles it; only a missing or invalid one asks whether it is needed
        // (which may read the branch's changed paths).
        if !check.valid && !self.spec_required(task).await.unwrap_or(true) {
            check.required = false;
            check.valid = true;
            check.problems.clear();
        }
        check
    }

    /// Reads and validates the task spec; registers a discovered spec path.
    async fn dlv_read_spec(&self, task: &TaskRecord) -> SpecCheck {
        self.publish_after_updates(task, Box::pin(self.read_spec_under_plan_lock(task)))
            .await
    }

    async fn read_spec_under_plan_lock(&self, task: &TaskRecord) -> SpecCheck {
        let Some(worktree) = task.worktree_path.as_deref().map(PathBuf::from) else {
            return SpecCheck::problem(
                task.spec_path.clone(),
                "The task has no worktree yet, so its spec cannot be read.".into(),
            );
        };
        // As in the periodic spec sync, order this read and its recorded status with parking.
        let runtime = self.runtime(&task.id);
        let _work = runtime.steps_lock.lock().await;
        let path = match self.task_spec_path(task, &worktree).await {
            Ok(path) => path,
            Err(dir) => {
                // No spec (any more) in a worktree that is there: nothing it said is left to
                // keep the task from Done (review of SPEC-491, m2).
                if worktree.is_dir() {
                    self.note_spec_status(&task.id, None).await;
                }
                return SpecCheck::problem(
                    None,
                    format!(
                        "No spec is registered for this task and no Markdown file under `{dir}` has `yulius-task: {}` frontmatter; write the task spec there.",
                        task.id
                    ),
                );
            }
        };
        match read_spec(&worktree, &path).await {
            Ok(text) => {
                let report = spec_format::validate_spec_for(&text, &task.id);
                self.note_spec_status(&task.id, report.status).await;
                SpecCheck {
                    path: Some(path),
                    valid: report.is_valid(),
                    problems: report.problems.iter().map(ToString::to_string).collect(),
                    goal: section_text(&text, "Goal"),
                    report: Some(report),
                    required: true,
                }
            }
            Err(message) => SpecCheck::problem(Some(path.clone()), format!("Spec {message}.")),
        }
    }

    /// Gives a task that has no spec path yet the next free `SPEC-NNN-<slug>.md` of its specs
    /// directory, when the repository numbers its specs that way: parallel tasks each took the
    /// same number (nine `SPEC-010`s on 2026-10-12). The number is given out like any numbered
    /// record's (SPEC-083 NUM-004): past the specs on the integration branch, in every live
    /// task's checkout and every number given out before.
    pub async fn allocate_spec_path(&self, task: &TaskRecord, worktree: &Path) {
        if task.spec_path.is_some() {
            return;
        }
        let (dir, found) = self.dlv_discover_spec(task, worktree).await;
        if found.is_some() || !safe_relative(&dir) {
            return;
        }
        // Only a repository that numbers its specs this way gets a number.
        let given = match self
            .reserve_record_number(task, &dir, Some("SPEC-"), false)
            .await
        {
            Ok(Some(given)) => given,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "allocating the spec number failed");
                return;
            }
        };
        let slug = branch_slug(task).unwrap_or("task");
        let path = format!("{dir}/{}-{slug}.md", given.formatted());
        let update = TaskFieldUpdate {
            spec_path: Some(path),
            ..TaskFieldUpdate::default()
        };
        if let Err(err) = self
            .deps
            .store
            .update_task(&task.id, &update, self.now())
            .await
        {
            tracing::warn!(task = %task.id, error = %err, "allocating the spec path failed");
        }
    }

    /// The repository path of the task's spec: the registered one while that file exists, else
    /// the one whose frontmatter names the task — registered on the way, so a renamed spec is
    /// followed instead of blocking the agent on a path it cannot change (F 2026-10-11).
    /// `Err(dir)` when there is none; `dir` is the specs directory searched.
    pub(crate) async fn task_spec_path(
        &self,
        task: &TaskRecord,
        worktree: &Path,
    ) -> Result<String, String> {
        if let Some(path) = &task.spec_path
            && safe_relative(path)
            && worktree.join(path).is_file()
        {
            return Ok(path.clone());
        }
        match self.dlv_discover_spec(task, worktree).await {
            (_, Some(found)) => {
                if task.spec_path.as_deref() != Some(found.as_str()) {
                    let update = TaskFieldUpdate {
                        spec_path: Some(found.clone()),
                        ..TaskFieldUpdate::default()
                    };
                    if let Err(err) = self
                        .deps
                        .store
                        .update_task(&task.id, &update, self.now())
                        .await
                    {
                        tracing::warn!(error = %err, "registering the discovered spec failed");
                    }
                }
                Ok(found)
            }
            (dir, None) => Err(dir),
        }
    }

    /// Looks for a spec naming this task under the project's specs directory, the file named
    /// with the branch slug first. Returns the directory searched and the repository-relative
    /// path found.
    async fn dlv_discover_spec(
        &self,
        task: &TaskRecord,
        worktree: &Path,
    ) -> (String, Option<String>) {
        let config = read_repo_file(
            worktree,
            "yulius.toml",
            u64::try_from(MAX_CONFIG_BYTES).unwrap_or(u64::MAX),
        )
        .await
        .ok();
        let layout = spec_format::project_layout(config.as_deref());
        let dir = layout.specs_dir.trim_end_matches('/').to_owned();
        if !safe_relative(&dir) {
            return (dir, None);
        }
        let Ok(mut entries) = tokio::fs::read_dir(worktree.join(&dir)).await else {
            return (dir, None);
        };
        let mut names = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            if names.len() >= MAX_SPEC_CANDIDATES {
                break;
            }
            let is_file = entry.file_type().await.is_ok_and(|t| t.is_file());
            if let Some(name) = entry.file_name().to_str()
                && is_file
                && name.ends_with(".md")
            {
                names.push(name.to_owned());
            }
        }
        let slug_name = branch_slug(task).map(|slug| format!("-{slug}.md"));
        let not_slug_named = |name: &String| {
            slug_name
                .as_deref()
                .is_none_or(|ending| !name.ends_with(ending))
        };
        names.sort();
        names.sort_by_key(not_slug_named);
        for name in names {
            let relative = format!("{dir}/{name}");
            let Ok(text) = read_spec(worktree, &relative).await else {
                continue;
            };
            if text.contains(task.id.as_str())
                && spec_format::validate_spec(&text).task_id.as_deref() == Some(task.id.as_str())
            {
                return (dir, Some(relative));
            }
        }
        (dir, None)
    }
}

pub(super) fn is_hex_sha(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The `type(scope)!` part of a conventional commit subject or title (`docs: …`,
/// `fix(ui)!: …`), when it has one.
pub(super) fn conventional_prefix(subject: &str) -> Option<&str> {
    let (head, rest) = subject.trim().split_once(": ")?;
    if rest.trim().is_empty() {
        return None;
    }
    let bare = head.strip_suffix('!').unwrap_or(head);
    let kind = match bare.split_once('(') {
        Some((kind, scope)) if scope.len() > 1 && scope.ends_with(')') => kind,
        Some(_) => return None,
        None => bare,
    };
    (!kind.is_empty() && kind.len() <= 10 && kind.bytes().all(|b| b.is_ascii_lowercase()))
        .then_some(head)
}

/// The `type(scope)!` head of `subject` the default title may take: under an enforced branch
/// `policy` only a conventional head of its types, else any conventional-looking one.
fn title_head(subject: &str, policy: &BranchPolicy) -> Option<String> {
    if policy.is_enforced() {
        policy.conventional_head(subject.trim()).map(str::to_owned)
    } else {
        conventional_prefix(subject).map(str::to_owned)
    }
}

/// How much a change of conventional `head`'s type says about a pull request, highest first:
/// feat, fix, perf, refactor, revert, build and ci, test, docs, style, chore; any other type
/// ranks lowest (SPEC-069 BPL-008).
fn type_rank(head: &str) -> u8 {
    let bare = head.strip_suffix('!').unwrap_or(head);
    match bare.split_once('(').map_or(bare, |(kind, _)| kind) {
        "feat" => 10,
        "fix" => 9,
        "perf" => 8,
        "refactor" => 7,
        "revert" => 6,
        "build" | "ci" => 5,
        "test" => 4,
        "docs" => 3,
        "style" => 2,
        "chore" => 1,
        _ => 0,
    }
}

/// The default title of a task's pull request: the task's title, prefixed — when it is not a
/// conventional title itself — with the type and scope of the branch's most significant
/// conventional commit (the oldest of the highest [`type_rank`]), so repositories that require
/// conventional titles accept it and the squash commit names the change, not its spec
/// ("Prepare this repository for Yulius" → "docs: prepare this repository for Yulius",
/// F 2026-10-11; a `docs(spec)` commit then `feat(doctor)` → "feat(doctor): …", SPEC-069
/// BPL-008). `subjects` are the branch's commit subjects, newest first. Under an enforced branch
/// `policy` only its types count, and without such a commit the branch's own type prefixes the
/// title (BPL-006); the caller clips it to the policy's length.
pub(super) fn default_pr_title(
    task_title: &str,
    subjects: &[String],
    branch: &str,
    policy: &BranchPolicy,
) -> String {
    let title = task_title.trim();
    if title_head(title, policy).is_some() {
        return title.to_owned();
    }
    let mut best: Option<String> = None;
    for head in subjects.iter().rev().filter_map(|s| title_head(s, policy)) {
        if best
            .as_deref()
            .is_none_or(|b| type_rank(&head) > type_rank(b))
        {
            best = Some(head);
        }
    }
    let prefix = best.or_else(|| policy.branch_type(branch).map(str::to_owned));
    prefixed_title(title, prefix.as_deref())
}

/// `title` after `prefix` and ": " (as is without one). "Prepare …" reads "prepare …" after the
/// prefix; an acronym ("API …") keeps its case.
fn prefixed_title(title: &str, prefix: Option<&str>) -> String {
    let Some(prefix) = prefix else {
        return title.to_owned();
    };
    let mut chars = title.chars();
    let first = chars.next();
    let second_lower = chars.clone().next().is_some_and(char::is_lowercase);
    let rest = chars.as_str();
    match first {
        Some(c) if c.is_uppercase() && second_lower => {
            format!("{prefix}: {}{rest}", c.to_lowercase())
        }
        Some(c) => format!("{prefix}: {c}{rest}"),
        None => title.to_owned(),
    }
}

/// A pull request title as Yulius sends it: one line, at most `max` characters.
pub(super) fn sent_title(title: &str, max: usize) -> String {
    clip_title(&title.replace(['\n', '\r'], " "), max)
}

/// The longest title Yulius gives a pull request under `policy`.
pub(super) fn max_title_chars(policy: &BranchPolicy) -> usize {
    if policy.is_enforced() {
        branch_policy::MAX_TITLE_CHARS
    } else {
        MAX_PR_TITLE_CHARS
    }
}

/// The title a task's pull request should carry before Yulius merges it (SPEC-069 BPL-008):
/// the default title of its final commits, when its title `current` is still one Yulius gave it
/// and differs. A title Yulius gave it is the task's title with the type of one of the branch's
/// commits, or with the branch's type when none has a type (the default of the commits there
/// were when it opened, by the rule then or now: commits are only added, never rewritten). A
/// title a person or an agent wrote is none of these and is kept. `None`: leave it.
pub(super) fn merge_title(
    current: &str,
    task_title: &str,
    subjects: &[String],
    branch: &str,
    policy: &BranchPolicy,
) -> Option<String> {
    let max = max_title_chars(policy);
    let current = current.trim();
    let task = task_title.trim();
    let gave = |prefix: Option<&str>| sent_title(&prefixed_title(task, prefix), max) == current;
    let given = if title_head(task, policy).is_some() {
        gave(None)
    } else {
        subjects
            .iter()
            .filter_map(|s| title_head(s, policy))
            .any(|head| gave(Some(&head)))
            || gave(policy.branch_type(branch))
    };
    let title = sent_title(&default_pr_title(task, subjects, branch, policy), max);
    (given && title != current).then_some(title)
}

/// `text` when it has at most `max` characters, else its first `max - 1` and "…".
pub(super) fn clip_title(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some(_) => clip_chars(text, max.saturating_sub(1)),
        None => text.to_owned(),
    }
}

pub(super) fn clip_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

pub(super) fn clip_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut cut = max;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    &text[..cut]
}

pub(crate) fn tail_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// A safe repository-relative path: non-empty, bounded, only normal components.
pub(super) fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH_BYTES
        && !path.contains('\0')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// A missing spec holds a code edit, while a spec that exists and cannot be judged
/// is left to the Stop gate (SPEC-426).
#[derive(Debug, thiserror::Error)]
pub(crate) enum RepoReadError {
    #[error("`{0}` does not exist in the task worktree")]
    Missing(String),
    #[error("`{path}` is too large (limit {max} bytes)")]
    TooLarge { path: String, max: u64 },
    #[error("{0}")]
    Unreadable(String),
}

impl RepoReadError {
    fn from_io(relative: &str, error: &std::io::Error) -> Self {
        if error.kind() == std::io::ErrorKind::NotFound {
            Self::Missing(relative.into())
        } else {
            Self::Unreadable(format!("`{relative}` cannot be read"))
        }
    }

    pub(super) fn from_open(relative: &str, err: AppError) -> Self {
        match err {
            AppError::NotFound => Self::Missing(relative.into()),
            AppError::Forbidden(message) if message == crate::files::OUTSIDE => {
                Self::Unreadable(format!("`{relative}` points outside the task worktree"))
            }
            _ => Self::Unreadable(format!("`{relative}` cannot be read")),
        }
    }
}

/// The slug of a task's `type/NNN-slug` branch: `cleanup-x` of `feat/651-cleanup-x`.
fn branch_slug(task: &TaskRecord) -> Option<&str> {
    task.branch
        .as_deref()
        .and_then(|b| b.split_once('/').map(|(_, rest)| rest))
        .and_then(|rest| rest.split_once('-').map(|(_, slug)| slug))
        .filter(|slug| !slug.is_empty())
}

/// Reads a whole spec through an in-worktree handle, refusing links and partial documents.
pub(crate) async fn read_spec(root: &Path, relative: &str) -> Result<String, RepoReadError> {
    read_checked_file(root, relative, MAX_SPEC_BYTES, true).await
}

/// Reads a repository file below `root`, refusing paths that escape it (also through
/// symlinks, including one swapped in after the check: the file is read through a handle whose
/// real path is inside `root`) and files larger than `max` bytes. Errors are user-facing and never
/// contain server paths.
pub(crate) async fn read_repo_file(
    root: &Path,
    relative: &str,
    max: u64,
) -> Result<String, String> {
    read_checked_file(root, relative, max, false)
        .await
        .map_err(|err| err.to_string())
}

async fn read_checked_file(
    root: &Path,
    relative: &str,
    max: u64,
    spec: bool,
) -> Result<String, RepoReadError> {
    use RepoReadError::{TooLarge, Unreadable};
    if !safe_relative(relative) {
        return Err(Unreadable(format!(
            "`{}` is not a safe repository-relative path",
            clip_chars(relative, 100)
        )));
    }
    let root = tokio::fs::canonicalize(root)
        .await
        .map_err(|_| Unreadable("the task worktree is missing".into()))?;
    let path = root.join(relative);
    if spec {
        let metadata = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(|err| RepoReadError::from_io(relative, &err))?;
        if metadata.is_symlink() {
            return Err(Unreadable(format!(
                "`{relative}` is a symbolic link. Use a regular file for the task spec"
            )));
        }
    }
    let resolved = tokio::fs::canonicalize(&path)
        .await
        .map_err(|err| RepoReadError::from_io(relative, &err))?;
    if !resolved.starts_with(&root) {
        return Err(Unreadable(format!(
            "`{relative}` points outside the task worktree"
        )));
    }
    // A spec is opened at its original leaf, so a link swapped in there cannot be followed.
    let path = if spec { path } else { resolved };
    crate::race::reached(crate::race::Point::RepoFileRead, &path);
    let opened = crate::files::open_inside(&root, &path, nix::fcntl::OFlag::O_RDONLY)
        .map_err(|err| RepoReadError::from_open(relative, err))?;
    let file = tokio::fs::File::from_std(std::fs::File::from(opened));
    let meta = file
        .metadata()
        .await
        .map_err(|err| RepoReadError::from_io(relative, &err))?;
    if !meta.is_file() {
        return Err(Unreadable(format!("`{relative}` is not a file")));
    }
    // Check the bytes actually read before decoding, including a file that grew since metadata().
    let mut bytes = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(
        &mut tokio::io::AsyncReadExt::take(file, max.saturating_add(1)),
        &mut bytes,
    )
    .await
    .map_err(|err| RepoReadError::from_io(relative, &err))?;
    if bytes.len() as u64 > max {
        return Err(TooLarge {
            path: relative.into(),
            max,
        });
    }
    String::from_utf8(bytes).map_err(|_| Unreadable(format!("`{relative}` is not UTF-8 text")))
}

/// Text of a level-2 section (outside code fences), trimmed.
pub(super) fn section_text(markdown: &str, name: &str) -> Option<String> {
    let mut inside = false;
    let mut fence = false;
    let mut out = Vec::new();
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = !fence;
        }
        if !fence && trimmed.starts_with('#') {
            let level = trimmed.bytes().take_while(|b| *b == b'#').count();
            if level <= 2 {
                if inside {
                    break;
                }
                inside = level == 2 && trimmed[level..].trim().eq_ignore_ascii_case(name);
                continue;
            }
        }
        if inside {
            out.push(line);
        }
    }
    let text = out.join("\n").trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// Pull request body: the caller's text, or the spec's Goal, path and Plan checklist and the
/// task steps; always ending with `link` back to the task. Bounded to [`MAX_PR_BODY_BYTES`].
pub(super) fn pull_request_body(
    link: &str,
    spec: &SpecCheck,
    steps: &[StepRecord],
    custom: Option<&str>,
) -> String {
    let mut body = String::new();
    match custom.map(str::trim).filter(|b| !b.is_empty()) {
        Some(custom) => body.push_str(custom),
        None => {
            if let Some(goal) = &spec.goal {
                body.push_str("## Goal\n\n");
                body.push_str(&clip_chars(goal, GOAL_EXCERPT_CHARS));
                body.push_str("\n\n");
            }
            if let Some(path) = &spec.path {
                body.push_str(&format!("**Spec:** `{path}`\n\n"));
            }
            if let Some(report) = &spec.report
                && !report.plan.is_empty()
            {
                body.push_str("## Plan\n\n");
                for item in &report.plan {
                    body.push_str(&format!(
                        "{}- [{}] {}\n",
                        "  ".repeat(item.depth.min(4)),
                        if item.checked { "x" } else { " " },
                        item.text
                    ));
                }
                body.push('\n');
            }
            if !steps.is_empty() {
                body.push_str("## Steps\n\n");
                for step in steps {
                    body.push_str(&format!("- {} ({})\n", step.title, step.status));
                }
                body.push('\n');
            }
        }
    }
    let room = MAX_PR_BODY_BYTES.saturating_sub(link.len() + 8);
    let mut out = clip_bytes(&body, room).trim_end().to_owned();
    out.push_str("\n\n---\n");
    out.push_str(link);
    out
}
