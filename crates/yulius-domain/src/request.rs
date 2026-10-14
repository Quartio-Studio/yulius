//! What a task asks its agent to do (SPEC-001 TSK-010, DLV-009; SPEC-011): the request with its
//! attached files, the definition of done Yulius holds the task to, and the one-line `/goal`
//! condition Claude Code keeps the session working towards.
//!
//! The request is written to a file the agent reads (no length limit), and the goal condition
//! names it: Claude Code caps a goal condition at 500 characters, so a long request is never the
//! condition itself. Attached files reach the session's first message natively: an image pasted
//! into the TUI (`[Image #N]`, SPEC-030, ADR-0026), another file mentioned as `@path` (SPEC-025,
//! ADR-0022).

/// Longest goal condition Yulius types (Claude Code caps goal conditions at 500 characters).
pub const GOAL_MAX_CHARS: usize = 450;

/// How a goal-mode session's first message (its attached files) begins.
const ATTACHMENTS_INTRO: &str = "The files attached to this task. Its goal follows in the next \
                                 message; until then, only look at them.";

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
    /// A playground (SPEC-099): its Git stays on the server; no Yulius CI, pull request or
    /// deploy, and `delivery_merge` squashes the task into the integration branch there.
    pub playground: bool,
}

/// How a session's first message reaches Claude Code (SPEC-030).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opening {
    /// The CLI prompt argument (the attached files, none an image, are `@path` mentions).
    Prompt(String),
    /// Typed into the TUI: each image's path pasted on its own (Claude Code attaches a pasted
    /// image natively, as `[Image #N]`; a mention would be a Read tool call), then the text
    /// pasted (empty when there is none), then Enter.
    Typed {
        /// Absolute paths of the attached images, in order.
        images: Vec<String>,
        /// The rest of the message, the other files mentioned.
        text: String,
    },
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
        // The text becomes a command-line argument: no escape sequence or NUL reaches it.
        let text: String = text
            .join("\n")
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect();
        let text = text.trim().to_owned();
        Self { text, attachments }
    }

    /// The first message of a session that is not in goal mode: the text, then a mention of
    /// each attached file.
    #[must_use]
    pub fn first_message(&self) -> String {
        if self.attachments.is_empty() {
            return self.text.clone();
        }
        let mentions = mentions(&self.attachments);
        if self.text.is_empty() {
            return mentions;
        }
        format!("{}\n\n{mentions}", self.text)
    }

    /// A fresh session's first message: in goal mode only when files are attached (its `/goal`
    /// follows), else the request. With an attached image it is typed, so the image is pasted.
    #[must_use]
    pub fn opening(&self, goal_mode: bool) -> Option<Opening> {
        let (images, others): (Vec<String>, Vec<String>) =
            self.attachments.iter().cloned().partition(|p| is_image(p));
        if images.is_empty() {
            return if goal_mode {
                self.attachments_message().map(Opening::Prompt)
            } else {
                Some(Opening::Prompt(self.first_message()))
            };
        }
        let rest = Self {
            text: self.text.clone(),
            attachments: others,
        };
        let text = if goal_mode {
            rest.attachments_message()
                .unwrap_or_else(|| ATTACHMENTS_INTRO.to_owned())
        } else {
            rest.first_message()
        };
        Some(Opening::Typed { images, text })
    }

    /// The first message of a goal-mode session, before Yulius types its `/goal`: the attached
    /// files, mentioned so Claude Code attaches them. `None` without attached files.
    #[must_use]
    pub fn attachments_message(&self) -> Option<String> {
        (!self.attachments.is_empty())
            .then(|| format!("{ATTACHMENTS_INTRO}\n\n{}", mentions(&self.attachments)))
    }
}

/// One mention per line, always quoted (`@"/a/b.png"`: Claude Code reads an unquoted mention
/// only up to its last letter or digit), which Claude Code attaches itself. A path no mention can
/// carry (a double quote, or `#`, which starts a line range) is listed to open with the Read
/// tool instead.
fn mentions(paths: &[String]) -> String {
    let (mentionable, unmentionable): (Vec<&String>, Vec<&String>) =
        paths.iter().partition(|p| !p.contains(['"', '#']));
    let mut lines: Vec<String> = mentionable.iter().map(|p| format!("@\"{p}\"")).collect();
    if !unmentionable.is_empty() {
        let mut list = String::from("Open with the Read tool:");
        for path in unmentionable {
            list.push_str("\n- ");
            list.push_str(path);
        }
        lines.push(list);
    }
    lines.join("\n")
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
    // A control character (an escape sequence, NUL) never reaches the agent's prompt as a path.
    (path.starts_with('/') && path.len() > 1 && !path.chars().any(char::is_control)).then_some(path)
}

fn attachments_section(paths: &[String]) -> String {
    let mut out = String::from(
        "These files were given in the session's first message; open one again with the Read \
         tool (it shows images) if you need it:",
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
                 after your own gates and the review, or work needing no change reported; closing \
                 summary given.",
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
                 Yulius CI, no pull request and no deploy. GitHub is not connected to it.\n\n\
                 - **If the request changes this repository:** work on this task's branch (it \
                 starts from `{integration}`); write the spec first where the repository's rules \
                 ask for one; implement with tests; run the project's gates yourself; get the \
                 independent review of the exact head; then merge into `{integration}` with \
                 `delivery_merge`, which squashes your branch onto it here. If it refuses, do what \
                 it says and call it again.\n\
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
    if done.playground {
        out.push('\n');
        return out;
    }
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

    #[test]
    fn the_first_message_mentions_the_files_so_claude_code_attaches_them() {
        // Always quoted: Claude Code reads an unquoted mention only up to its last letter or
        // digit (`notes.`, `c++` would lose their ends).
        let request = Request::parse("Look\n@/srv/a.png\n@'/srv/my shot.png'\n@/srv/c++");
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
            odd.ends_with("Open with the Read tool:\n- /srv/say\"hi\".png\n- /srv/issue#3.png"),
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
    fn a_goal_session_first_gets_the_attached_files_when_there_are_any() {
        assert_eq!(Request::parse("Only text").attachments_message(), None);
        let message = Request::parse("Make it blue\n@/srv/a.png\n@/srv/b.txt")
            .attachments_message()
            .unwrap_or_default();
        assert!(
            message.starts_with(
                "The files attached to this task. Its goal follows in the next message"
            ),
            "{message}"
        );
        assert!(
            message.ends_with("\n\n@\"/srv/a.png\"\n@\"/srv/b.txt\""),
            "{message}"
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
        let request = Request::parse("Look\n@/srv/notes.txt");
        assert_eq!(
            request.opening(false),
            Some(Opening::Prompt("Look\n\n@\"/srv/notes.txt\"".into()))
        );
        assert_eq!(
            request.opening(true),
            request.attachments_message().map(Opening::Prompt)
        );
        assert_eq!(Request::parse("Only text").opening(true), None);
        assert_eq!(
            Request::parse("Only text").opening(false),
            Some(Opening::Prompt("Only text".into()))
        );
    }

    #[test]
    fn images_are_pasted_and_other_files_mentioned_in_a_typed_first_message() {
        // SPEC-030: an `@` mention reaches the model as a Read tool call; a pasted image path
        // is attached natively (`[Image #1]`).
        let request = Request::parse("Make it blue\n@/srv/a.png\n@'/srv/my shot.JPG'\n@/srv/b.txt");
        assert_eq!(
            request.opening(false),
            Some(Opening::Typed {
                images: vec!["/srv/a.png".into(), "/srv/my shot.JPG".into()],
                text: "Make it blue\n\n@\"/srv/b.txt\"".into(),
            })
        );
        let Some(Opening::Typed { images, text }) = request.opening(true) else {
            panic!("a goal task with images types its first message");
        };
        assert_eq!(images, ["/srv/a.png", "/srv/my shot.JPG"]);
        assert!(
            text.starts_with("The files attached to this task. Its goal follows"),
            "{text}"
        );
        assert!(text.ends_with("\n\n@\"/srv/b.txt\""), "{text}");
        // Only images: nothing to mention, the text is the request (or empty).
        assert_eq!(
            Request::parse("@/srv/a.png").opening(false),
            Some(Opening::Typed {
                images: vec!["/srv/a.png".into()],
                text: String::new(),
            })
        );
        let Some(Opening::Typed { text, .. }) = Request::parse("@/srv/a.png").opening(true) else {
            panic!("a goal task with an image types its first message");
        };
        assert!(!text.contains('@'), "{text}");
    }

    #[test]
    fn a_typed_first_message_carries_no_control_character() {
        let request = Request::parse("Look\u{1b}[201~ here\n@/srv/a.png\n@/srv/b\u{1b}.png");
        let Some(Opening::Typed { images, text }) = request.opening(false) else {
            panic!("an image is attached");
        };
        assert_eq!(images, ["/srv/a.png"]);
        assert!(!text.contains('\u{1b}'), "{text:?}");
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
        assert!(
            file.contains(
                "## Attached files\n\nThese files were given in the session's first message"
            ),
            "{file}"
        );
        assert!(file.contains("\n- /srv/a.png\n"), "{file}");
        assert!(file.contains("`delivery_merge` on green Yulius CI"));
        assert!(file.contains("Then deploy to `dev` with `delivery_deploy`"));
        assert!(file.contains("**Never promote to `main`**"));
        assert!(file.contains("`yulius wait` with `run_in_background`"));
    }

    /// SPEC-099 PLY-006: a playground's tasks merge into its own `dev` with `delivery_merge`; there
    /// is no Yulius CI, pull request or deploy, and they are told so.
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
        assert!(!short.contains("Yulius CI"), "{short}");
        let full = playground.full();
        assert!(full.contains("This sandbox is a playground"), "{full}");
        assert!(
            full.contains("no Yulius CI, no pull request and no deploy"),
            "{full}"
        );
        assert!(full.contains("run the project's gates yourself"), "{full}");
        assert!(
            full.contains("merge into `dev` with `delivery_merge`"),
            "{full}"
        );
        assert!(!full.contains("on green Yulius CI"), "{full}");
        assert!(!full.contains("delivery_deploy"), "{full}");
        let file = request_markdown(3, "x", "a", &Request::parse("x"), &playground);
        assert!(!file.contains("yulius wait"), "nothing to wait for: {file}");
    }
}
