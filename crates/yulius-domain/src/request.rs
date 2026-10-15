//! What a task asks its agent to do (SPEC-001 TSK-010, DLV-009; SPEC-011): the request with its
//! attached files, the definition of done Yulius holds the task to, and the one-line `/goal`
//! condition Claude Code keeps the session working towards.
//!
//! The request is written to a file the agent reads (no length limit), and the goal condition
//! names it, so a long request is never the condition itself. A goal session's only first message
//! is its `/goal` (SPEC-117, ADR-0095): the condition names the attached files, which the agent
//! opens with its Read tool, since a `/goal` carries no image block. In prompt mode the attached
//! files reach the first message natively: an image pasted into the TUI (`[Image #N]`, SPEC-030,
//! ADR-0026), another file mentioned as `@path` (SPEC-025, ADR-0022).
//!
//! A file keeps its place in the text (SPEC-124, ADR-0101): the composer writes it there as a
//! quoted mention `@'/path'`, and the request file and the first message carry it at that place.

/// Longest goal condition Yulius gives (Claude Code 2.1.292 caps goal conditions at 4000
/// characters; half of it leaves room for a lower cap again).
pub const GOAL_MAX_CHARS: usize = 2000;

/// Longest request text quoted inside the goal condition; a longer one is only named by its file.
const INLINE_REQUEST_MAX_CHARS: usize = 150;

/// The delivery a task's work goes through, as far as its definition of done needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoneDefinition {
    /// Branch that receives task pull requests (`dev`).
    pub integration_branch: String,
    /// Branch that represents production (`main`).
    pub production_branch: String,
    /// Environments after integration that an agent deploys and verifies (they have a command).
    pub integration_environments: Vec<String>,
    /// A playground (SPEC-099): its Git stays on the server; no pull request or deploy, and
    /// `delivery_merge` squashes the task into the integration branch there once Yulius CI passed
    /// on it (SPEC-163).
    pub playground: bool,
}

/// How a prompt-mode session's first message reaches Claude Code (SPEC-030).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opening {
    /// The CLI prompt argument (the attached files, none an image, are `@path` mentions).
    Prompt(String),
    /// Typed into the TUI: the message pasted in order, each image's path pasted on its own
    /// where it sits in the text (Claude Code attaches a pasted image natively, as `[Image #N]`
    /// at the cursor; a mention would be a Read tool call), the other files mentioned in the
    /// text; then Enter (SPEC-124).
    Typed {
        /// The pastes, in order; never two text pastes in a row, never an empty one.
        pastes: Vec<Paste>,
    },
}

/// One paste of a typed first message (SPEC-124).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Paste {
    /// Text, with the attached files that are not images mentioned in it.
    Text(String),
    /// The absolute path of an attached image.
    Image(String),
}

/// Whether Claude Code takes a pasted path as an image (Claude Code 2.1.292:
/// `/\.(png|jpe?g|gif|webp)$/i`).
#[must_use]
pub fn is_image(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// One piece of a request, in the order it was written (SPEC-124).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// Text as written, without control characters (tabs and newlines kept).
    Text(String),
    /// Attached file `n` of [`Request::attachments`], at this place in the text.
    File(usize),
}

/// A task's request split into its text and the files attached to it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Request {
    /// What the user wrote, without the attached files (a space left doubled by a file taken out
    /// of a line becomes one).
    pub text: String,
    /// Absolute paths of the attached files, in order.
    pub attachments: Vec<String>,
    /// The request in order: text and each attached file where it sits. Blank text at either end
    /// is left out.
    pub parts: Vec<Part>,
}

impl Request {
    /// Splits a prompt (SPEC-124 INL-006). A quoted mention `@'/path'` anywhere in a line (the
    /// composer writes each file where it sits in the text, always quoted) is a file at that
    /// place; a line that is only an `@path` or `@'path'` mention (the composer once added one
    /// per file after the text, shell-quoted when the path has spaces) is a file at that line.
    /// The rest is text.
    #[must_use]
    pub fn parse(prompt: &str) -> Self {
        let mut parts = Vec::new();
        let mut attachments = Vec::new();
        let mut lines = Vec::new();
        for (i, line) in prompt.lines().enumerate() {
            if i > 0 {
                push_text(&mut parts, "\n");
            }
            if let Some(path) = attachment_path(line.trim()) {
                parts.push(Part::File(attachments.len()));
                attachments.push(path);
                continue;
            }
            // The line without its files; a space doubled by a file taken out becomes one.
            let mut kept = String::new();
            let keep = |kept: &mut String, piece: &str, after_file: bool| {
                let doubled = after_file
                    && kept.ends_with(char::is_whitespace)
                    && piece.starts_with(char::is_whitespace);
                let skip = if doubled {
                    piece.chars().next().map_or(0, char::len_utf8)
                } else {
                    0
                };
                kept.push_str(&piece[skip..]);
            };
            let mut rest = line;
            let mut after_file = false;
            while let Some(at) = rest.find("@'") {
                let (before, mention) = rest.split_at(at);
                if let Some((path, used)) = quoted_path(&mention[2..]) {
                    push_text(&mut parts, before);
                    keep(&mut kept, before, after_file);
                    parts.push(Part::File(attachments.len()));
                    attachments.push(path);
                    after_file = true;
                    rest = &mention[2 + used..];
                } else {
                    push_text(&mut parts, &rest[..at + 2]);
                    keep(&mut kept, &rest[..at + 2], after_file);
                    after_file = false;
                    rest = &mention[2..];
                }
            }
            push_text(&mut parts, rest);
            keep(&mut kept, rest, after_file);
            lines.push(kept);
        }
        // Blank text at either end is not part of the request.
        if let Some(Part::Text(first)) = parts.first_mut() {
            *first = first.trim_start().to_owned();
        }
        if let Some(Part::Text(last)) = parts.last_mut() {
            *last = last.trim_end().to_owned();
        }
        parts.retain(|part| !matches!(part, Part::Text(text) if text.is_empty()));
        // The text becomes a command-line argument: no escape sequence or NUL reaches it.
        let text = without_controls(&lines.join("\n")).trim().to_owned();
        Self {
            text,
            attachments,
            parts,
        }
    }

    /// The first message of a prompt-mode session: the text with each attached file mentioned
    /// where it sits (SPEC-124 INL-007).
    #[must_use]
    pub fn first_message(&self) -> String {
        self.render(&self.parts, mention)
    }

    /// A fresh prompt-mode session's first message, the request. With an attached image it is
    /// typed, so each image is pasted where it sits. (A goal-mode session's first message is its
    /// `/goal`, [`goal_condition`].)
    #[must_use]
    pub fn opening(&self) -> Opening {
        let image = |part: &Part| match part {
            Part::File(n) => self.attachments.get(*n).filter(|p| is_image(p)),
            Part::Text(_) => None,
        };
        if !self.parts.iter().any(|part| image(part).is_some()) {
            return Opening::Prompt(self.first_message());
        }
        let mut pastes = Vec::new();
        let mut run = Vec::new();
        let flush = |run: &mut Vec<Part>, pastes: &mut Vec<Paste>| {
            let text = self.render(run, mention);
            // Claude Code puts a space after a pasted image's `[Image #N]`: not a second one.
            let text = match pastes.last() {
                Some(Paste::Image(_)) => text.strip_prefix(' ').unwrap_or(&text),
                _ => &text,
            };
            if !text.is_empty() {
                pastes.push(Paste::Text(text.to_owned()));
            }
            run.clear();
        };
        for part in &self.parts {
            match image(part) {
                Some(path) => {
                    // A mention just before keeps a space: the pasted path never joins it.
                    if matches!(run.last(), Some(Part::File(_))) {
                        run.push(Part::Text(" ".into()));
                    }
                    flush(&mut run, &mut pastes);
                    pastes.push(Paste::Image(path.clone()));
                }
                None => run.push(part.clone()),
            }
        }
        flush(&mut run, &mut pastes);
        Opening::Typed { pastes }
    }

    /// The request text with each file inside it written by `file`, up to the last text: files
    /// after all the text are left to a list of their own, as before SPEC-124.
    fn inline_text(&self, file: impl Fn(usize, &str) -> String) -> String {
        let end = self
            .parts
            .iter()
            .rposition(|part| matches!(part, Part::Text(text) if !text.trim().is_empty()))
            .map_or(0, |i| i + 1);
        self.render_with(&self.parts[..end], file)
            .trim_end()
            .to_owned()
    }

    /// `parts` as text, each file written by `file` (from its path).
    fn render(&self, parts: &[Part], file: impl Fn(&str) -> String) -> String {
        self.render_with(parts, |_, path| file(path))
    }

    /// `parts` as text, each file written by `file` (from its number and path), kept off the
    /// word before it and a word after it by a space (an `@` mention needs one before it).
    fn render_with(&self, parts: &[Part], file: impl Fn(usize, &str) -> String) -> String {
        let mut out = String::new();
        for (i, part) in parts.iter().enumerate() {
            match part {
                Part::Text(text) => out.push_str(text),
                Part::File(n) => {
                    let Some(path) = self.attachments.get(*n) else {
                        continue;
                    };
                    if out.ends_with(|c: char| !c.is_whitespace()) {
                        out.push(' ');
                    }
                    out.push_str(&file(*n, path));
                    if let Some(Part::Text(next)) = parts.get(i + 1)
                        && next.starts_with(char::is_alphanumeric)
                    {
                        out.push(' ');
                    }
                }
            }
        }
        out
    }
}

/// Appends text to the parts, joined to text just before it, without control characters.
fn push_text(parts: &mut Vec<Part>, text: &str) {
    let text = without_controls(text);
    if text.is_empty() {
        return;
    }
    if let Some(Part::Text(last)) = parts.last_mut() {
        last.push_str(&text);
    } else {
        parts.push(Part::Text(text));
    }
}

/// `text` without control characters but tabs and newlines.
fn without_controls(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

/// An attached file mentioned for Claude Code, always quoted (`@"/a/b.png"`: Claude Code reads
/// an unquoted mention only up to its last letter or digit), which it attaches itself. A path no
/// mention can carry (a double quote, or `#`, which starts a line range) is named for the Read
/// tool instead.
fn mention(path: &str) -> String {
    if path.contains(['"', '#']) {
        format!("[open with the Read tool: {path}]")
    } else {
        format!("@\"{path}\"")
    }
}

/// The path of an `@path` attachment line (`@/a/b.png` or `@'/a/b c.png'`), if the line is one.
fn attachment_path(line: &str) -> Option<String> {
    let rest = line.strip_prefix('@')?;
    if let Some(quoted) = rest.strip_prefix('\'') {
        return quoted_path(quoted).and_then(|(path, used)| (used == quoted.len()).then_some(path));
    }
    if rest.chars().any(char::is_whitespace) {
        return None;
    }
    valid_path(rest.to_owned())
}

/// The path of a POSIX single-quoted mention after its opening quote (`'\''` closes, adds a
/// quote and reopens), with the bytes it takes, closing quote included.
fn quoted_path(quoted: &str) -> Option<(String, usize)> {
    let mut path = String::new();
    let mut rest = quoted;
    loop {
        let end = rest.find('\'')?;
        path.push_str(&rest[..end]);
        rest = &rest[end + 1..];
        match rest.strip_prefix("\\''") {
            Some(after) => {
                path.push('\'');
                rest = after;
            }
            None => break,
        }
    }
    valid_path(path).map(|path| (path, quoted.len() - rest.len()))
}

/// `path` when it is absolute and holds no control character (an escape sequence or a NUL never
/// reaches the agent's prompt as a path).
fn valid_path(path: String) -> Option<String> {
    (path.starts_with('/') && path.len() > 1 && !path.chars().any(char::is_control)).then_some(path)
}

fn attachments_section(paths: &[String]) -> String {
    let mut out = String::from(
        "These files are attached to this task; open each with the Read tool (it shows \
         images):",
    );
    for path in paths {
        out.push_str("\n- ");
        out.push_str(path);
    }
    out
}

/// One line of text: control characters and runs of whitespace become single spaces.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

impl DoneDefinition {
    /// The definition of done in one short sentence (part of the goal condition).
    #[must_use]
    pub fn short(&self) -> String {
        if self.playground {
            return format!(
                "Done = repository changes merged into {} of this playground with delivery_merge \
                 on green Yulius CI after your own gates and the review, or work needing no change \
                 reported; closing summary given.",
                self.integration_branch
            );
        }
        let deploy = if self.integration_environments.is_empty() {
            String::new()
        } else {
            format!(
                ", deployed to {} and verified",
                self.integration_environments.join(", ")
            )
        };
        format!(
            "Done = repository changes merged into {} on green Yulius CI{deploy}, or work needing \
             no change reported; closing summary given; never promote to {} unless asked.",
            self.integration_branch, self.production_branch
        )
    }

    /// The definition of done in full (the request file).
    #[must_use]
    pub fn full(&self) -> String {
        let integration = &self.integration_branch;
        if self.playground {
            return format!(
                "Yulius holds this task to it. Work on your own until it is met; do not stop to \
                 ask anyone in between.\n\n\
                 This sandbox is a playground: its Git stays on this server, and there is no \
                 pull request and no deploy. GitHub is not connected to it.\n\n\
                 - **If the request changes this repository:** work on this task's branch (it \
                 starts from `{integration}`); write the spec first where the repository's rules \
                 ask for one; implement with tests; run the project's gates yourself; get the \
                 independent review of the exact head; then merge into `{integration}` with \
                 `delivery_merge`. Yulius CI tests your head merged into `{integration}` first \
                 (when the project has CI), and `delivery_merge` squashes your branch onto it here \
                 once that passed; while it runs, the merge is armed. If it refuses, do what it \
                 says and call it again.\n\
                 - **If it needs no repository change** (research, a brainstorm, an answer): do it \
                 and report the result.\n\
                 - **End with a short summary:** what changed, how it was verified, and what could \
                 not be tested in this environment."
            );
        }
        let mut out = format!(
            "Yulius holds this task to it. Work on your own until it is met; do not stop to ask \
             anyone in between.\n\n\
             - **If the request changes this repository:** work on this task's branch (it starts \
             from `{integration}`); write the spec first where the repository's rules ask for one; \
             implement with tests; get the independent review of the exact head; merge into \
             `{integration}` with `delivery_merge` on green Yulius CI (while the checks still \
             run, Yulius merges by itself once they pass)."
        );
        if !self.integration_environments.is_empty() {
            let envs = self
                .integration_environments
                .iter()
                .map(|e| format!("`{e}`"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                " Then deploy to {envs} with `delivery_deploy` and verify it there."
            ));
        }
        out.push_str(&format!(
            " Fix what fails and loop until it is green; Yulius deletes the merged branch.\n\
             - **If it needs no repository change** (research, a brainstorm, an answer, setting \
             something up outside the repository): do it and report the result.\n\
             - **Never promote to `{}`** or deploy to production unless the request asks for it: \
             a person does that in a separate task.\n\
             - **End with a short summary:** what changed, how it was verified, and what could \
             not be tested in this environment.",
            self.production_branch
        ));
        out
    }
}

/// What a message queued into a task that has not started adds to its request (SPEC-052
/// QMS-003): a blank line, who added it and when (`at`, already formatted), a blank line and the
/// trimmed text. Later additions follow earlier ones, so a later line corrects an earlier one.
/// `None` for a message with no text.
#[must_use]
pub fn queued_addition(login: &str, at: &str, text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| {
        format!(
            "\n\n**Added by {} before the task started ({}):**\n\n{text}",
            one_line(login),
            one_line(at)
        )
    })
}

/// The request file a task's agent reads first.
#[must_use]
pub fn request_markdown(
    number: u32,
    title: &str,
    author: &str,
    request: &Request,
    done: &DoneDefinition,
) -> String {
    let mut out = format!(
        "# Task #{number}: {}\n\nRequested by {author}.\n\n## Request\n\n",
        one_line(title)
    );
    // Each file inside the text is written where it sits, by its number in the list below
    // (SPEC-124 INL-007).
    let text = request.inline_text(|n, path| format!("[file {}: {path}]", n + 1));
    if text.is_empty() {
        out.push_str("(no text: see the attached files)\n");
    } else {
        out.push_str(&text);
        out.push('\n');
    }
    if !request.attachments.is_empty() {
        out.push_str("\n## Attached files\n\n");
        out.push_str(&attachments_section(&request.attachments));
        out.push('\n');
    }
    out.push_str("\n## Definition of done\n\n");
    out.push_str(&done.full());
    out.push_str(
        "\n\n## Waiting\n\nWhen only Yulius CI or a person can move the task forward, start \
         `yulius wait` with `run_in_background` and end your turn: it returns when the checks at \
         your head finish, your delivery changes or a merge you armed is decided. Never poll with \
         `sleep`, `pgrep -f` or monitor loops.\n",
    );
    out
}

/// The `/goal` condition for a task: the request itself when it is short (with no attached
/// file), else the request file and the attached files (when they fit), followed by the
/// definition of done. Never longer than [`GOAL_MAX_CHARS`].
#[must_use]
pub fn goal_condition(request: &Request, request_path: &str, done: &DoneDefinition) -> String {
    let text = one_line(&request.text);
    let done = done.short();
    let inline = format!("{text} | {done} | Full request: {request_path}");
    if request.attachments.is_empty()
        && !text.is_empty()
        && text.chars().count() <= INLINE_REQUEST_MAX_CHARS
        && inline.chars().count() <= GOAL_MAX_CHARS
    {
        return inline;
    }
    let named = format!("Everything requested in {request_path} is done. {done}");
    if !request.attachments.is_empty() {
        // As they are: an attachment path holds no control character (`attachment_path`).
        let files = request.attachments.join(", ");
        let with_files = format!(
            "Everything requested in {request_path} is done. Attached files (open each with the \
             Read tool; it shows images): {files}. {done}"
        );
        if with_files.chars().count() <= GOAL_MAX_CHARS {
            return with_files;
        }
    }
    if named.chars().count() <= GOAL_MAX_CHARS {
        return named;
    }
    // An unusually long path or branch names: keep the request file, cut the rest.
    named.chars().take(GOAL_MAX_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queued_message_follows_the_request_under_who_added_it_and_when() {
        let added = queued_addition("alice", "2026-10-13 10:42 UTC", "  Keep the logo.\n")
            .unwrap_or_default();
        assert_eq!(
            added,
            "\n\n**Added by alice before the task started (2026-10-13 10:42 UTC):**\n\nKeep the logo."
        );
        // It joins the request text the agent gets, attachment lines in it included.
        let request = Request::parse(&format!("Make it blue{added}\n@/srv/a.png"));
        assert!(
            request.text.ends_with("10:42 UTC):**\n\nKeep the logo."),
            "{request:?}"
        );
        assert_eq!(request.attachments, vec!["/srv/a.png".to_owned()]);
        assert_eq!(queued_addition("alice", "now", " \n\t "), None);
        // A login never breaks the line it is named on.
        assert!(
            queued_addition("a\nb", "now", "x")
                .unwrap_or_default()
                .contains("Added by a b before")
        );
    }

    fn done(envs: &[&str]) -> DoneDefinition {
        DoneDefinition {
            integration_branch: "dev".into(),
            production_branch: "main".into(),
            integration_environments: envs.iter().map(|e| (*e).to_owned()).collect(),
            playground: false,
        }
    }

    const PATH: &str =
        "/var/lib/yulius/sandboxes/d62be476-f169-4960-b45c-0fce704d71f9/home/.yulius/tasks/17.md";

    #[test]
    fn attachment_lines_are_split_from_the_text() {
        let prompt = "Fix the header\n\n@/srv/a/b.png\n@'/srv/a/my shot.png'\n@'/srv/it'\\''s.png'";
        let request = Request::parse(prompt);
        assert_eq!(request.text, "Fix the header");
        assert_eq!(
            request.attachments,
            ["/srv/a/b.png", "/srv/a/my shot.png", "/srv/it's.png"]
        );
        // A mention inside a sentence, a relative path or an e-mail is text.
        let text = Request::parse("Ask @anna\n@ docs\n@relative/path.png\nmail me@x.io");
        assert!(text.attachments.is_empty());
        assert_eq!(text.text.lines().count(), 4);
    }

    /// SPEC-124 INL-006: a quoted mention inside a line is a file at that place; a whole-line
    /// mention is a file at that line; the text is what remains, without doubled spaces.
    #[test]
    fn a_mention_inside_the_text_is_a_file_at_its_place() {
        let request = Request::parse(
            "This part @'/srv/a.png' looks wrong,\nwhile here@'/srv/my b.png'.\n@/srv/c.txt",
        );
        assert_eq!(
            request.attachments,
            ["/srv/a.png", "/srv/my b.png", "/srv/c.txt"]
        );
        assert_eq!(
            request.parts,
            [
                Part::Text("This part ".into()),
                Part::File(0),
                Part::Text(" looks wrong,\nwhile here".into()),
                Part::File(1),
                Part::Text(".\n".into()),
                Part::File(2),
            ]
        );
        assert_eq!(request.text, "This part looks wrong,\nwhile here.");
        // An escaped quote inside the path, and a file first.
        let quoted = Request::parse("@'/srv/it'\\''s.png' first");
        assert_eq!(quoted.attachments, ["/srv/it's.png"]);
        assert_eq!(quoted.parts, [Part::File(0), Part::Text(" first".into())]);
        assert_eq!(quoted.text, "first");
        // Not a file: a relative path, an unclosed quote, an unquoted mention inside a line, a
        // control character in the path. They stay text.
        for text in [
            "see @'srv/a.png' now",
            "see @'/srv/a.png now",
            "see @/srv/a.png now",
            "see @'/srv/a\u{1b}.png' now",
        ] {
            let request = Request::parse(text);
            assert!(request.attachments.is_empty(), "{text}: {request:?}");
            assert_eq!(request.parts.len(), 1, "{text}: {request:?}");
        }
    }

    /// SPEC-124 INL-006: an older prompt (files after the text) parses into the same order.
    #[test]
    fn an_older_prompt_has_its_files_after_the_text() {
        let request = Request::parse("Fix the header\n\n@/srv/a/b.png\n@'/srv/a/my shot.png'\n");
        assert_eq!(request.text, "Fix the header");
        assert_eq!(
            request.parts,
            [
                Part::Text("Fix the header\n\n".into()),
                Part::File(0),
                Part::Text("\n".into()),
                Part::File(1),
            ]
        );
        assert_eq!(
            Request::parse("  Just text \n").parts,
            [Part::Text("Just text".into())]
        );
        assert!(Request::parse(" \n ").parts.is_empty());
    }

    /// SPEC-124 INL-007: prompt mode mentions each file where it sits, a space keeping a mention
    /// off the word before or after it.
    #[test]
    fn the_first_message_mentions_each_file_in_place() {
        let request = Request::parse("This part@'/srv/a.png'looks wrong, see @'/srv/b.txt'.");
        assert_eq!(
            request.first_message(),
            "This part @\"/srv/a.png\" looks wrong, see @\"/srv/b.txt\"."
        );
        // An older prompt's message is what it was.
        assert_eq!(
            Request::parse("Look\n\n@/srv/a.txt\n@'/srv/my b.txt'").first_message(),
            "Look\n\n@\"/srv/a.txt\"\n@\"/srv/my b.txt\""
        );
        // A path no mention can carry is named in place for the Read tool.
        assert_eq!(
            Request::parse("Look @'/srv/issue#3.png' here").first_message(),
            "Look [open with the Read tool: /srv/issue#3.png] here"
        );
    }

    /// SPEC-124 INL-007: a typed first message is pasted in order: text, each image where it
    /// sits, the rest; other files mentioned in the text.
    #[test]
    fn a_typed_first_message_pastes_each_image_where_it_sits() {
        let request = Request::parse(
            "This part @'/srv/a.png' looks wrong, see @'/srv/n.txt'\nwhile here @'/srv/b.png'",
        );
        assert_eq!(
            request.opening(),
            Opening::Typed {
                pastes: vec![
                    Paste::Text("This part ".into()),
                    Paste::Image("/srv/a.png".into()),
                    // Claude Code adds the space after an image itself.
                    Paste::Text("looks wrong, see @\"/srv/n.txt\"\nwhile here ".into()),
                    Paste::Image("/srv/b.png".into()),
                ],
            }
        );
    }

    /// SPEC-124 review: a mention right before a pasted image keeps a space, so the pasted
    /// path never joins the mention.
    #[test]
    fn a_mention_before_an_image_keeps_a_space() {
        let request = Request::parse("See @'/srv/n.txt'@'/srv/a.png' here");
        assert_eq!(
            request.opening(),
            Opening::Typed {
                pastes: vec![
                    Paste::Text("See @\"/srv/n.txt\" ".into()),
                    Paste::Image("/srv/a.png".into()),
                    Paste::Text("here".into()),
                ],
            }
        );
    }

    /// SPEC-124 INL-007: an older prompt written without the composer's blank line keeps its
    /// files where they are, on the lines after the text (it once got a blank line added).
    #[test]
    fn an_older_prompt_without_a_blank_line_keeps_its_lines() {
        assert_eq!(
            Request::parse("Look\n@/srv/a.txt").first_message(),
            "Look\n@\"/srv/a.txt\""
        );
    }

    /// SPEC-124 INL-007: the request file writes each file inside the text where it sits, by its
    /// number in the attached files list; files after all the text are only listed, as before.
    #[test]
    fn the_request_file_marks_each_file_in_place() {
        let request = Request::parse("This part @'/srv/a.png' looks wrong.\n\n@/srv/b.png");
        let file = request_markdown(5, "x", "a", &request, &done(&[]));
        assert!(
            file.contains(
                "## Request\n\nThis part [file 1: /srv/a.png] looks wrong.\n\n## Attached files"
            ),
            "{file}"
        );
        assert!(file.contains("\n- /srv/a.png\n- /srv/b.png\n"), "{file}");
        let older = request_markdown(
            5,
            "x",
            "a",
            &Request::parse("Look\n\n@/srv/b.png"),
            &done(&[]),
        );
        assert!(
            older.contains("## Request\n\nLook\n\n## Attached files"),
            "{older}"
        );
    }

    #[test]
    fn the_first_message_mentions_the_files_so_claude_code_attaches_them() {
        // Always quoted: Claude Code reads an unquoted mention only up to its last letter or
        // digit (`notes.`, `c++` would lose their ends).
        let request = Request::parse("Look\n\n@/srv/a.png\n@'/srv/my shot.png'\n@/srv/c++");
        assert_eq!(
            request.first_message(),
            "Look\n\n@\"/srv/a.png\"\n@\"/srv/my shot.png\"\n@\"/srv/c++\""
        );
        assert_eq!(
            Request::parse("@/srv/a.png").first_message(),
            "@\"/srv/a.png\""
        );
        assert_eq!(Request::parse("Just text").first_message(), "Just text");
        // A path no mention can carry (a double quote, or `#`, which Claude Code reads as a
        // line range) is left for the Read tool.
        let odd = Request::parse("Look\n@/srv/say\"hi\".png\n@/srv/issue#3.png").first_message();
        assert!(!odd.contains('@'), "{odd}");
        assert!(
            odd.ends_with(
                "[open with the Read tool: /srv/say\"hi\".png]\n[open with the Read tool: /srv/issue#3.png]"
            ),
            "{odd}"
        );
    }

    #[test]
    fn control_characters_never_reach_the_first_message() {
        let request = Request::parse("Look\n@/srv/a\u{1b}[2J.png\n@/srv/b\u{0}.png\tok");
        assert!(request.attachments.is_empty(), "{request:?}");
        // Such a line stays text, without its control characters (tabs and newlines kept).
        let first = request.first_message();
        assert!(!first.contains(['\u{1b}', '\u{0}']), "{first:?}");
        assert!(
            first.contains("@/srv/a[2J.png\n@/srv/b.png\tok"),
            "{first:?}"
        );
    }

    #[test]
    fn images_are_the_files_claude_code_pastes_as_images() {
        for image in ["/s/a.png", "/s/b.JPG", "/s/c.jpeg", "/s/d.gif", "/s/e.WebP"] {
            assert!(is_image(image), "{image}");
        }
        for other in ["/s/a.svg", "/s/b.txt", "/s/png", "/s/c.png.zip", "/s/d.bmp"] {
            assert!(!is_image(other), "{other}");
        }
    }

    #[test]
    fn without_images_the_first_message_stays_the_command_line_prompt() {
        let request = Request::parse("Look\n\n@/srv/notes.txt");
        assert_eq!(
            request.opening(),
            Opening::Prompt("Look\n\n@\"/srv/notes.txt\"".into())
        );
        assert_eq!(
            Request::parse("Only text").opening(),
            Opening::Prompt("Only text".into())
        );
    }

    #[test]
    fn images_are_pasted_and_other_files_mentioned_in_a_typed_first_message() {
        // SPEC-030: an `@` mention reaches the model as a Read tool call; a pasted image path
        // is attached natively (`[Image #1]`).
        // An older prompt's images follow its text, where they are (SPEC-124 INL-007).
        let request = Request::parse("Make it blue\n@/srv/a.png\n@'/srv/my shot.JPG'\n@/srv/b.txt");
        assert_eq!(
            request.opening(),
            Opening::Typed {
                pastes: vec![
                    Paste::Text("Make it blue\n".into()),
                    Paste::Image("/srv/a.png".into()),
                    Paste::Text("\n".into()),
                    Paste::Image("/srv/my shot.JPG".into()),
                    Paste::Text("\n@\"/srv/b.txt\"".into()),
                ],
            }
        );
        // Only images: nothing but their pastes.
        assert_eq!(
            Request::parse("@/srv/a.png").opening(),
            Opening::Typed {
                pastes: vec![Paste::Image("/srv/a.png".into())],
            }
        );
    }

    #[test]
    fn a_typed_first_message_carries_no_control_character() {
        let request = Request::parse("Look\u{1b}[201~ here\n@/srv/a.png\n@/srv/b\u{1b}.png");
        let Opening::Typed { pastes } = request.opening() else {
            panic!("an image is attached");
        };
        assert_eq!(
            pastes
                .iter()
                .filter(|p| matches!(p, Paste::Image(_)))
                .collect::<Vec<_>>(),
            [&Paste::Image("/srv/a.png".into())]
        );
        for paste in &pastes {
            if let Paste::Text(text) = paste {
                assert!(!text.contains('\u{1b}'), "{text:?}");
            }
        }
    }

    #[test]
    fn a_short_request_is_the_goal_with_the_definition_of_done() {
        let goal = goal_condition(
            &Request::parse("Stopped tasks flash Working, fix"),
            PATH,
            &done(&[]),
        );
        assert!(goal.starts_with("Stopped tasks flash Working, fix | Done = repository changes merged into dev on green Yulius CI, or work"));
        assert!(goal.contains("never promote to main unless asked"));
        assert!(goal.ends_with(PATH));
        assert!(
            goal.chars().count() <= GOAL_MAX_CHARS,
            "{}",
            goal.chars().count()
        );
    }

    #[test]
    fn a_long_request_or_one_with_files_is_named_by_its_file() {
        let long = Request::parse(&"word ".repeat(200));
        for request in [long, Request::parse("Short\n@/srv/a.png")] {
            let goal = goal_condition(&request, PATH, &done(&["dev"]));
            assert!(goal.starts_with(&format!("Everything requested in {PATH} is done.")));
            assert!(goal.contains(" Done = "), "{goal}");
            assert!(goal.contains("deployed to dev and verified"));
            assert!(
                goal.chars().count() <= GOAL_MAX_CHARS,
                "{}",
                goal.chars().count()
            );
        }
        // Never over the cap, whatever the names.
        let huge = DoneDefinition {
            integration_branch: "b".repeat(300),
            ..done(&[])
        };
        assert!(
            goal_condition(&Request::parse("x"), PATH, &huge)
                .chars()
                .count()
                <= GOAL_MAX_CHARS
        );
    }

    /// SPEC-117 ONE-002: the goal is the session's only first message, so it names the attached
    /// files (images too: a `/goal` carries no image block), as plain paths in order.
    #[test]
    fn the_goal_names_the_attached_files() {
        let request =
            Request::parse("Make it blue\n@/srv/a.png\n@'/srv/my shot.JPG'\n@/srv/notes.txt");
        let goal = goal_condition(&request, PATH, &done(&[]));
        assert_eq!(
            goal,
            format!(
                "Everything requested in {PATH} is done. Attached files (open each with the Read \
                 tool; it shows images): /srv/a.png, /srv/my shot.JPG, /srv/notes.txt. {}",
                done(&[]).short()
            )
        );
        assert!(!goal.contains('@'), "{goal}");
        // A path is named as it is, its spaces kept.
        let spaced = goal_condition(&Request::parse("Look\n@'/srv/a  b.png'"), PATH, &done(&[]));
        assert!(spaced.contains(": /srv/a  b.png. Done"), "{spaced}");
        // Without files, a short request is still quoted and a long one named.
        assert!(!goal_condition(&Request::parse("Short"), PATH, &done(&[])).contains("Attached"));
    }

    #[test]
    fn attached_files_that_would_pass_the_cap_are_left_to_the_request_file() {
        let many: String = (0..40)
            .map(|i| format!("\n@/srv/attachments/{}/shot-{i}.png", "x".repeat(60)))
            .collect();
        let goal = goal_condition(&Request::parse(&format!("Look{many}")), PATH, &done(&[]));
        assert_eq!(
            goal,
            format!(
                "Everything requested in {PATH} is done. {}",
                done(&[]).short()
            )
        );
        // A few files of a real sandbox fit.
        let few: String = (0..4)
            .map(|i| {
                format!(
                    "\n@/var/lib/yulius/sandboxes/{}/attachments/pasted-{i}.png",
                    "d".repeat(36)
                )
            })
            .collect();
        let goal = goal_condition(
            &Request::parse(&format!("Look{few}")),
            PATH,
            &done(&["dev"]),
        );
        assert!(goal.contains("pasted-3.png. Done = "), "{goal}");
        assert!(goal.chars().count() <= GOAL_MAX_CHARS);
    }

    #[test]
    fn the_request_file_has_the_request_the_files_and_the_definition_of_done() {
        let request = Request::parse("Make it fast\nreally\n@/srv/a.png");
        let file = request_markdown(17, "Speed\nup", "mkarp", &request, &done(&["dev"]));
        assert!(file.starts_with("# Task #17: Speed up\n\nRequested by mkarp."));
        assert!(file.contains("## Request\n\nMake it fast\nreally\n"));
        assert!(
            file.contains(
                "## Attached files\n\nThese files are attached to this task; open each with the \
                 Read tool (it shows images):"
            ),
            "{file}"
        );
        assert!(file.contains("\n- /srv/a.png\n"), "{file}");
        assert!(file.contains("`delivery_merge` on green Yulius CI"));
        assert!(file.contains("Then deploy to `dev` with `delivery_deploy`"));
        assert!(file.contains("**Never promote to `main`**"));
        assert!(file.contains("`yulius wait` with `run_in_background`"));
    }

    /// SPEC-099 PLY-006, SPEC-163 PCI-007: a playground's tasks merge into its own `dev` with
    /// `delivery_merge` on green Yulius CI; there is no pull request or deploy, and they are told
    /// so, and how to wait.
    #[test]
    fn a_playground_done_definition_merges_locally() {
        let playground = DoneDefinition {
            playground: true,
            ..done(&["dev"])
        };
        let short = playground.short();
        assert!(
            short.starts_with(
                "Done = repository changes merged into dev of this playground with delivery_merge"
            ),
            "{short}"
        );
        assert!(short.contains("on green Yulius CI"), "{short}");
        let full = playground.full();
        assert!(full.contains("This sandbox is a playground"), "{full}");
        assert!(full.contains("no pull request and no deploy"), "{full}");
        assert!(
            full.contains("Yulius CI tests your head merged into `dev`"),
            "{full}"
        );
        assert!(!full.contains("no Yulius CI"), "{full}");
        assert!(full.contains("run the project's gates yourself"), "{full}");
        assert!(
            full.contains("merge into `dev` with `delivery_merge`"),
            "{full}"
        );
        assert!(!full.contains("delivery_deploy"), "{full}");
        let file = request_markdown(3, "x", "a", &Request::parse("x"), &playground);
        assert!(file.contains("`yulius wait`"), "{file}");
    }
}
