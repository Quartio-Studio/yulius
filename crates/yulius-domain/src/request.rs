//! What a task asks its agent to do (SPEC-001 TSK-010, DLV-009; SPEC-011): the request with its
//! attached files, the definition of done Yulius holds the task to, and the one-line `/goal`
//! condition Claude Code keeps the session working towards.
//!
//! The request is written to a file the agent reads (no length limit, attachments listed as
//! files to open), and the goal condition names it: Claude Code caps a goal condition at 500
//! characters, so a long request is never the condition itself.

/// Longest goal condition Yulius types (Claude Code caps goal conditions at 500 characters).
pub const GOAL_MAX_CHARS: usize = 450;

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
}

/// A task's request split into its text and the files attached to it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Request {
    /// What the user wrote, without the attachment lines.
    pub text: String,
    /// Absolute paths of the attached files, in order.
    pub attachments: Vec<String>,
}

impl Request {
    /// Splits a prompt: every line that is only an `@path` mention (the composer adds one per
    /// attached file, shell-quoted when the path has spaces) is an attachment; the rest is text.
    #[must_use]
    pub fn parse(prompt: &str) -> Self {
        let mut text = Vec::new();
        let mut attachments = Vec::new();
        for line in prompt.lines() {
            match attachment_path(line.trim()) {
                Some(path) => attachments.push(path),
                None => text.push(line),
            }
        }
        let text = text.join("\n").trim().to_owned();
        Self { text, attachments }
    }

    /// The first message of a session that is not in goal mode: the text, then the attached
    /// files to open.
    #[must_use]
    pub fn first_message(&self) -> String {
        if self.attachments.is_empty() {
            return self.text.clone();
        }
        format!(
            "{}\n\n{}",
            self.text,
            attachments_section(&self.attachments)
        )
    }
}

/// The path of an `@path` attachment line (`@/a/b.png` or `@'/a/b c.png'`), if the line is one.
fn attachment_path(line: &str) -> Option<String> {
    let rest = line.strip_prefix('@')?;
    let path = if let Some(quoted) = rest.strip_prefix('\'') {
        // POSIX single quotes; `'\''` closes, adds a quote and reopens.
        let mut out = String::new();
        let mut chars = quoted.chars().peekable();
        loop {
            match chars.next()? {
                '\'' => {
                    if chars.peek().is_none() {
                        break;
                    }
                    let escape: String = chars.by_ref().take(3).collect();
                    if escape != "\\''" {
                        return None;
                    }
                    out.push('\'');
                }
                c => out.push(c),
            }
        }
        out
    } else {
        if rest.chars().any(char::is_whitespace) {
            return None;
        }
        rest.to_owned()
    };
    (path.starts_with('/') && path.len() > 1).then_some(path)
}

fn attachments_section(paths: &[String]) -> String {
    let mut out = String::from(
        "Attached files: open each one with the Read tool (it shows images) before you start:",
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
    if request.text.is_empty() {
        out.push_str("(no text: see the attached files)\n");
    } else {
        out.push_str(&request.text);
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
/// file), else the request file, followed by the definition of done. Never longer than
/// [`GOAL_MAX_CHARS`].
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
    if named.chars().count() <= GOAL_MAX_CHARS {
        return named;
    }
    // An unusually long path or branch names: keep the request file, cut the rest.
    named.chars().take(GOAL_MAX_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(envs: &[&str]) -> DoneDefinition {
        DoneDefinition {
            integration_branch: "dev".into(),
            production_branch: "main".into(),
            integration_environments: envs.iter().map(|e| (*e).to_owned()).collect(),
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

    #[test]
    fn the_first_message_lists_the_files_to_open() {
        let request = Request::parse("Look\n@/srv/a.png");
        let first = request.first_message();
        assert!(first.starts_with("Look\n\nAttached files: open each one with the Read tool"));
        assert!(first.ends_with("\n- /srv/a.png"));
        assert_eq!(Request::parse("Just text").first_message(), "Just text");
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
            assert!(goal.starts_with(&format!("Everything requested in {PATH} is done. Done =")));
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

    #[test]
    fn the_request_file_has_the_request_the_files_and_the_definition_of_done() {
        let request = Request::parse("Make it fast\nreally\n@/srv/a.png");
        let file = request_markdown(17, "Speed\nup", "mkarp", &request, &done(&["dev"]));
        assert!(file.starts_with("# Task #17: Speed up\n\nRequested by mkarp."));
        assert!(file.contains("## Request\n\nMake it fast\nreally\n"));
        assert!(file.contains("## Attached files\n\nAttached files: open each one"));
        assert!(file.contains("`delivery_merge` on green Yulius CI"));
        assert!(file.contains("Then deploy to `dev` with `delivery_deploy`"));
        assert!(file.contains("**Never promote to `main`**"));
        assert!(file.contains("`yulius wait` with `run_in_background`"));
    }
}
