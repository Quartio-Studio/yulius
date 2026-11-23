//! Brokered commands (SPEC-626, ADR-0459): the rules of a project's command definitions. A
//! command is a CLI that needs an agent secret as a file; agents run it by name and Yulius runs
//! it outside their sandbox. This module checks what an administrator defines.

/// Longest command name.
pub const MAX_NAME: usize = 40;
/// Longest binary path, in bytes.
pub const MAX_BINARY: usize = 512;
/// Most entries of a definition's list (`fixed`, `subcommands`, `refuse`, `send_flags`, `env`,
/// `tasks`).
pub const MAX_ENTRIES: usize = 64;
/// Longest entry of a definition's list, in bytes.
pub const MAX_ENTRY: usize = 4096;
/// The longest description, in bytes.
pub const MAX_DESCRIPTION: usize = 2000;
/// Approval minutes: 1 to this.
pub const MAX_APPROVAL_MINUTES: u32 = 60;
/// The approval minutes when none are given.
pub const DEFAULT_APPROVAL_MINUTES: u32 = 10;
/// A run's time limit: 1 second to this.
pub const MAX_TIMEOUT_SECONDS: u32 = 1800;
/// The time limit when none is given.
pub const DEFAULT_TIMEOUT_SECONDS: u32 = 600;

/// The run's key file.
pub const KEYFILE: &str = "{keyfile}";
/// The run's fresh home.
pub const HOME: &str = "{home}";
/// The key's value, in `env` only.
pub const SECRET: &str = "{secret}";

/// When a run waits for a person (BRK-013).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approve {
    /// Every run.
    Always,
    /// A run the agent says changes something, or with one of the send flags.
    Send,
    /// No run.
    Never,
}

impl Approve {
    /// Parses `always`, `send` or `never`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "always" => Some(Self::Always),
            "send" => Some(Self::Send),
            "never" => Some(Self::Never),
            _ => None,
        }
    }

    /// Its name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Send => "send",
            Self::Never => "never",
        }
    }
}

/// Why a command name is refused (BRK-001): 1 to 40 of `a-z`, `0-9` and `-`, not starting with
/// `-`.
#[must_use]
pub fn name_refusal(name: &str) -> Option<String> {
    let valid = !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    (!valid).then(|| {
        format!(
            "a command's name is 1 to {MAX_NAME} lowercase letters, digits and dashes, not \
             starting with a dash"
        )
    })
}

/// Folders whose files no binary is taken from: what the kernel makes up, and the people's and
/// services' own (a brokered run's binary is the host's, ADR-0459).
const NOT_BINARY_DIRS: &[&str] = &[
    "/proc/",
    "/sys/",
    "/dev/",
    "/run/",
    "/tmp/",
    "/var/tmp/",
    "/home/",
    "/root/",
];

/// Why a binary path is refused (BRK-001): absolute and normalised, with no `.`, `..` or empty
/// component, at most [`MAX_BINARY`] bytes of printable ASCII without spaces, and not in a folder
/// of `NOT_BINARY_DIRS`. The caller refuses the data directory too.
#[must_use]
pub fn binary_refusal(binary: &str) -> Option<String> {
    let Some(rest) = binary.strip_prefix('/') else {
        return Some("the binary is an absolute path, like /usr/local/bin/deploy".into());
    };
    if binary.len() > MAX_BINARY || !binary.bytes().all(|b| b.is_ascii_graphic()) {
        return Some(format!(
            "the binary is a path of at most {MAX_BINARY} bytes of printable ASCII, without \
             spaces"
        ));
    }
    if let Some(dir) = NOT_BINARY_DIRS.iter().find(|d| binary.starts_with(**d)) {
        return Some(format!(
            "the binary is the host's own program, not one in {dir}"
        ));
    }
    if rest.is_empty()
        || rest
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == "..")
    {
        return Some("the binary's path has no empty, . or .. part".into());
    }
    None
}

/// Why an environment variable's name is refused (BRK-001): `[A-Za-z_][A-Za-z0-9_]*`, and not
/// one Yulius sets or one that changes how programs load (`PATH`, `HOME`, `TMPDIR`, `LD_*`,
/// `YULIUS_*`).
#[must_use]
pub fn env_name_refusal(name: &str) -> Option<String> {
    let mut bytes = name.bytes();
    let valid = bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && name.len() <= 128;
    if !valid {
        return Some(format!("{} is not a variable name", shown(name)));
    }
    let upper = name.to_ascii_uppercase();
    if matches!(upper.as_str(), "PATH" | "HOME" | "TMPDIR")
        || upper.starts_with("LD_")
        || upper.starts_with("YULIUS_")
    {
        return Some(format!("{name} is set by Yulius"));
    }
    None
}

/// Why a fixed argument or env value is refused for its placeholders (BRK-001): `{keyfile}` and
/// `{home}` anywhere, `{secret}` only in env (`in_env`), no other `{…}` placeholder.
#[must_use]
pub fn placeholder_refusal(value: &str, in_env: bool) -> Option<String> {
    if value.len() > MAX_ENTRY || value.contains('\0') {
        return Some(format!("a value is at most {MAX_ENTRY} bytes, without NUL"));
    }
    // Wherever it stands, also inside other braces (review round 1).
    if !in_env && value.contains(SECRET) {
        return Some("{secret} goes only in env: arguments show in a process list".into());
    }
    // Every `{` starts a candidate, so one nested in another brace is read too.
    for (start, _) in value.match_indices('{') {
        let after = &value[start + 1..];
        let Some(end) = after.find(['}', '{']) else {
            break;
        };
        if after[end..].starts_with('{') {
            continue;
        }
        let word = &after[..end];
        let token = &value[start..start + end + 2];
        let known = token == KEYFILE || token == HOME || (in_env && token == SECRET);
        let shaped = !word.is_empty()
            && word
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b':' || b == b'-');
        if !known && shaped {
            return Some(format!(
                "{token} is not a placeholder: use {{keyfile}}, {{home}} or, in env, {{secret}}"
            ));
        }
    }
    None
}

/// A flag's or subcommand's own characters: ASCII letters, digits, `_`, `.`, `:` and `-`.
fn word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-')
}

/// Why a flag of `refuse` or `send_flags` is refused: one or two dashes, then a letter or digit
/// and `word_char` characters (no space, `=` or invisible character, which would never match).
#[must_use]
pub fn flag_refusal(flag: &str) -> Option<String> {
    let name = flag.strip_prefix("--").or_else(|| flag.strip_prefix('-'));
    let valid = flag.len() <= 128
        && name.is_some_and(|n| {
            n.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric()) && n.bytes().all(word_char)
        });
    (!valid).then(|| format!("{} is not a flag like -k or --key-file", shown(flag)))
}

/// Why a subcommand of the allowed list is refused: a word that does not start with `-`, has no
/// `/` and no space.
#[must_use]
pub fn subcommand_refusal(word: &str) -> Option<String> {
    let valid = word.len() <= 128
        && word
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && word.bytes().all(word_char);
    (!valid).then(|| format!("{} is not a subcommand", shown(word)))
}

/// A value shown in a refusal: at most 60 characters.
fn shown(text: &str) -> String {
    let cut: String = text.chars().take(60).collect();
    format!("{cut:?}")
}

/// `value` with `{keyfile}` and `{home}` replaced, and `{secret}` when `secret` is given (env
/// only, BRK-009).
#[must_use]
pub fn expand(value: &str, keyfile: &str, home: &str, secret: Option<&str>) -> String {
    let value = value.replace(KEYFILE, keyfile).replace(HOME, home);
    match secret {
        Some(secret) => value.replace(SECRET, secret),
        None => value,
    }
}

/// Most arguments an agent passes to one run.
pub const MAX_ARGS: usize = 64;
/// The most of each stream a run returns, in bytes (BRK-010).
pub const OUTPUT_SHOWN: usize = 96 * 1024;

/// Whether `arg` sets `flag` (BRK-006), in every form a parser reads it. A long flag
/// (`--key-file`, or Go's one-dash `-address`) as `--name`, `--name=…`, an abbreviation a GNU or
/// Python parser takes (`--key-f`), and the one-dash form (`-key-file`, `-address=…`); a
/// two-character `-k` in any cluster of short flags that holds `k` (`-k`, `-kVALUE`, `-vk`).
#[must_use]
pub fn sets_flag(arg: &str, flag: &str) -> bool {
    if !arg.starts_with('-') || arg == "-" || arg == "--" {
        return false;
    }
    let short = flag.len() == 2 && !flag.starts_with("--");
    if short {
        let letter = &flag[1..];
        return !arg.starts_with("--") && arg[1..].contains(letter);
    }
    let long = flag.trim_start_matches('-');
    let (name, _) = arg.split_once('=').unwrap_or((arg, ""));
    if let Some(given) = name.strip_prefix("--") {
        return !given.is_empty() && long.starts_with(given);
    }
    name.trim_start_matches('-') == long
}

/// A character a `/` may stand between with `slashes`: `owner/repo`, `group/sub/name`.
fn beside_slash(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

/// Whether `arg` names a path (BRK-006): anything that starts with `~`, holds `=~`, `@~`, `..` or
/// `file:`, and any `/`; with `slashes`, a `/` passes only between two letters, digits, `_`, `.`
/// or `-` of a value (`owner/repo`, `--repo=owner/repo`), never in a short flag or a long flag
/// without `=`.
fn path_refusal(arg: &str, slashes: bool) -> bool {
    let tilde = arg.starts_with('~') || arg.contains("=~") || arg.contains("@~");
    let dots = arg.contains("..");
    let file = arg.to_ascii_lowercase().contains("file:");
    if tilde || dots || file {
        return true;
    }
    if !arg.contains('/') {
        return false;
    }
    if !slashes {
        return true;
    }
    let value = if arg.starts_with("--") {
        match arg.split_once('=') {
            Some((_, value)) => value,
            None => return true,
        }
    } else if arg.starts_with('-') {
        return true;
    } else {
        arg
    };
    let chars: Vec<char> = value.chars().collect();
    chars.iter().enumerate().any(|(i, c)| {
        *c == '/'
            && !(i > 0
                && i + 1 < chars.len()
                && beside_slash(chars[i - 1])
                && beside_slash(chars[i + 1]))
    }) || arg.split_once('=').is_some_and(|(_, v)| v.contains('='))
}

/// The most bytes of all of a run's arguments: the process host takes 256 KiB of argv in all.
pub const MAX_ARGS_BYTES: usize = 64 * 1024;

/// Why an agent's arguments are refused (BRK-006), if they are: their count and size, control
/// characters, paths, flags Yulius owns, and a subcommand outside the allowed ones.
#[must_use]
pub fn args_refusal(
    args: &[String],
    refuse: &[String],
    subcommands: &[String],
    slashes: bool,
) -> Option<String> {
    // Arguments are named by their place, never quoted: a refusal is audited, and an argument
    // may hold a value (review round 1 of slice 2).
    if args.len() > MAX_ARGS {
        return Some(format!("a run takes at most {MAX_ARGS} arguments"));
    }
    if args.iter().map(String::len).sum::<usize>() > MAX_ARGS_BYTES {
        return Some(format!(
            "a run's arguments are at most {} KiB in all",
            MAX_ARGS_BYTES / 1024
        ));
    }
    for (place, arg) in args.iter().enumerate() {
        let n = place + 1;
        if arg.len() > MAX_ENTRY {
            return Some(format!("argument {n} is over {MAX_ENTRY} bytes"));
        }
        if arg.chars().any(char::is_control) {
            return Some(format!("argument {n} holds a control character"));
        }
        if path_refusal(arg, slashes) {
            return Some(format!(
                "argument {n} looks like a file path, which is not allowed: the command cannot \
                 be pointed at another file"
            ));
        }
        if let Some(flag) = refuse.iter().find(|flag| sets_flag(arg, flag)) {
            return Some(format!(
                "argument {n} sets {flag}, which Yulius sets, not you"
            ));
        }
    }
    // The first argument itself, so no flag's value is read as the subcommand (review round 1).
    if !subcommands.is_empty() {
        let first = args.first().map_or("", String::as_str);
        if !subcommands.iter().any(|s| s == first) {
            return Some(format!(
                "the first argument must be an allowed subcommand ({})",
                subcommands.join(", ")
            ));
        }
    }
    None
}

/// Whether a run waits for a person (BRK-013).
#[must_use]
pub fn needs_approval(
    approve: Approve,
    send_flags: &[String],
    args: &[String],
    changes: bool,
) -> bool {
    match approve {
        Approve::Always => true,
        Approve::Never => false,
        Approve::Send => {
            changes
                || args
                    .iter()
                    .any(|arg| send_flags.iter().any(|flag| sets_flag(arg, flag)))
        }
    }
}

/// A word as a person reads it in a command line: bare when it is plain, else single-quoted.
fn quoted(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:@,+%{}".contains(&b));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// The full command line a person approves (BRK-014): the binary, the fixed arguments with
/// their placeholders as written, then the agent's.
#[must_use]
pub fn command_line(binary: &str, fixed: &[String], args: &[String]) -> String {
    std::iter::once(binary)
        .chain(fixed.iter().map(String::as_str))
        .chain(args.iter().map(String::as_str))
        .map(quoted)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The bytes `c` takes in a JSON string: an MCP answer carries both streams in one record,
/// which Yulius withholds past 256 KiB.
fn json_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if u32::from(c) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// A stream as it is shown (BRK-010): what fits in [`OUTPUT_SHOWN`] bytes once written as a JSON
/// string, cut at a character, with how many bytes of the whole stream (`total`, at least the
/// text's own) were left out.
#[must_use]
pub fn shown_output(text: &str, total: u64) -> String {
    let total = total.max(text.len() as u64);
    let mut size = 0;
    let mut end = text.len();
    for (at, c) in text.char_indices() {
        size += json_len(c);
        if size > OUTPUT_SHOWN {
            end = at;
            break;
        }
    }
    if end == text.len() && total == text.len() as u64 {
        return text.to_owned();
    }
    format!("{}\n[… {} bytes more]", &text[..end], total - end as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in ["deploy", "gcloud-2", "a", "0x"] {
            assert_eq!(name_refusal(ok), None, "{ok}");
        }
        for bad in ["", "-x", "Deploy", "de ploy", "de/ploy", &"a".repeat(41)] {
            assert!(name_refusal(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn binaries() {
        assert_eq!(binary_refusal("/usr/local/bin/deploy"), None);
        for bad in [
            "deploy",
            "./deploy",
            "~/bin/deploy",
            "/",
            "/usr//bin/x",
            "/usr/../bin/sh",
            "/usr/./bin/x",
            "/usr/bin/",
            "/usr/bin/x\n",
            "/usr/bin/a b",
            "/usr/bin/x\u{200b}",
            "/tmp/deploy",
            "/proc/self/exe",
            "/home/alice/bin/deploy",
        ] {
            assert!(binary_refusal(bad).is_some(), "{bad:?}");
        }
        assert!(binary_refusal(&format!("/{}", "a".repeat(600))).is_some());
    }

    #[test]
    fn env_names() {
        for ok in ["GOOGLE_APPLICATION_CREDENTIALS", "_X", "api_url"] {
            assert_eq!(env_name_refusal(ok), None, "{ok}");
        }
        for bad in [
            "",
            "1X",
            "A-B",
            "A B",
            "PATH",
            "home",
            "TMPDIR",
            "LD_PRELOAD",
            "ld_library_path",
            "YULIUS_TOKEN",
        ] {
            assert!(env_name_refusal(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn placeholders() {
        assert_eq!(placeholder_refusal("--key-file={keyfile}", false), None);
        assert_eq!(placeholder_refusal("{home}/.config", false), None);
        assert_eq!(placeholder_refusal("Bearer {secret}", true), None);
        // JSON and braces that are no placeholder stay as they are.
        assert_eq!(placeholder_refusal("{\"a\": 1}", false), None);
        assert_eq!(placeholder_refusal("{}", false), None);
        assert_eq!(placeholder_refusal("x{", false), None);
        let secret = placeholder_refusal("--token={secret}", false);
        assert!(secret.is_some_and(|r| r.contains("only in env")));
        assert!(placeholder_refusal("{url:api}", false).is_some());
        // Nested in other braces (review round 1).
        assert!(placeholder_refusal("{{secret}", false).is_some());
        assert!(placeholder_refusal("{\"t\":\"{secret}\"}", false).is_some());
        assert!(placeholder_refusal("{{url:api}}", true).is_some());
        assert!(placeholder_refusal("{\"t\":\"{other}\"}", true).is_some());
        assert_eq!(placeholder_refusal("{\"t\":\"{keyfile}\"}", false), None);
        assert!(placeholder_refusal("{key}", true).is_some());
        assert!(placeholder_refusal("a\0b", false).is_some());
    }

    #[test]
    fn flags_and_subcommands() {
        for ok in ["-k", "--key-file", "--send"] {
            assert_eq!(flag_refusal(ok), None, "{ok}");
        }
        for bad in [
            "k",
            "-",
            "--",
            "--a=b",
            "--a b",
            "-k\u{200b}",
            "---k",
            "-\u{e9}",
        ] {
            assert!(flag_refusal(bad).is_some(), "{bad}");
        }
        assert_eq!(subcommand_refusal("push"), None);
        for bad in ["", "-x", "a/b", "a b", "~", "push\u{200b}"] {
            assert!(subcommand_refusal(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn expansion() {
        assert_eq!(
            expand("--key={keyfile} {home} {secret}", "/k", "/h", None),
            "--key=/k /h {secret}"
        );
        assert_eq!(expand("T={secret}", "/k", "/h", Some("v")), "T=v");
    }

    #[test]
    fn approve_modes() {
        for mode in ["always", "send", "never"] {
            assert_eq!(Approve::parse(mode).map(Approve::as_str), Some(mode));
        }
        assert_eq!(Approve::parse("ask"), None);
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn flags_are_found_in_every_form_a_parser_reads() {
        for arg in [
            "--key-file",
            "--key-file=x",
            "--key-f",
            "--key=x",
            "-key-file",
            "--key-file=",
        ] {
            assert!(sets_flag(arg, "--key-file"), "{arg}");
        }
        for arg in ["--keys", "--other", "-k", "key-file", "--", "-", "-key"] {
            assert!(!sets_flag(arg, "--key-file"), "{arg}");
        }
        // Go's one-dash long flags, read in both forms (review round 1 of slice 2).
        for arg in [
            "-address",
            "-address=x",
            "--address",
            "--address=evil.example.com",
            "--addr",
        ] {
            assert!(sets_flag(arg, "-address"), "{arg}");
        }
        for arg in ["-a", "-addressx", "--other", "-addr"] {
            assert!(!sets_flag(arg, "-address"), "{arg}");
        }
        for arg in ["-k", "-kVALUE", "-vk", "-vkx"] {
            assert!(sets_flag(arg, "-k"), "{arg}");
        }
        for arg in ["--k", "--kv", "-v", "k", "-"] {
            assert!(!sets_flag(arg, "-k"), "{arg}");
        }
    }

    #[test]
    fn arguments_naming_a_path_or_an_owned_flag_are_refused() {
        let refuse = strings(&["--key-file", "-k"]);
        let subs = strings(&["status", "push"]);
        for args in [
            &["status", "/etc/passwd"][..],
            &["status", "~/.ssh/id_rsa"],
            &["status", "~"],
            &["status", "a/b"],
            &["status", ".."],
            &["status", "--x=~"],
            &["status", "file:key"],
            &["status", "--key-file", "x"],
            &["status", "--key-f=x"],
            &["status", "-key-file"],
            &["status", "-kx"],
            &["status", "-vk"],
            &["rm"],
            &["--format", "status", "rm"],
            &[],
            &["status", "a\nb"],
        ] {
            let args = strings(args);
            assert!(
                args_refusal(&args, &refuse, &subs, false).is_some(),
                "{args:?}"
            );
        }
        assert_eq!(
            args_refusal(
                &strings(&["status", "--json", "app-1"]),
                &refuse,
                &subs,
                false
            ),
            None
        );
        assert!(args_refusal(&strings(&["x"; 65]), &[], &[], false).is_some());
    }

    #[test]
    fn slashes_pass_only_inside_a_value() {
        for ok in ["owner/repo", "--repo=owner/repo", "a/b/c"] {
            assert_eq!(args_refusal(&strings(&[ok]), &[], &[], true), None, "{ok}");
        }
        for bad in [
            "/etc/passwd",
            "~/x",
            "a/../b",
            "--key=/etc/x",
            "--key=~/x",
            "-f/etc/x",
            "--repo",
            "host:/etc/passwd",
            "file:///etc/passwd",
            "https://example.com/x",
            "a//b",
            "a/",
            "x@/y",
            "a,/etc/passwd",
            "--values=x,/etc/passwd",
            "x;/etc/passwd",
            "a /etc/passwd",
            "[/etc/hosts]",
            "--a=b=/c",
        ] {
            let refused = if bad == "--repo" {
                args_refusal(&strings(&["--repo/x"]), &[], &[], true)
            } else {
                args_refusal(&strings(&[bad]), &[], &[], true)
            };
            assert!(refused.is_some(), "{bad}");
        }
    }

    #[test]
    fn approval_follows_the_mode_the_flags_and_the_agents_word() {
        let send = strings(&["--send", "-y"]);
        assert!(needs_approval(Approve::Always, &send, &[], false));
        assert!(!needs_approval(
            Approve::Never,
            &send,
            &strings(&["--send"]),
            true
        ));
        assert!(!needs_approval(
            Approve::Send,
            &send,
            &strings(&["status"]),
            false
        ));
        assert!(needs_approval(
            Approve::Send,
            &send,
            &strings(&["status"]),
            true
        ));
        assert!(needs_approval(
            Approve::Send,
            &strings(&["-send"]),
            &strings(&["push", "--send"]),
            false
        ));
        for flagged in ["--send", "--send=1", "--se", "-y", "-vy"] {
            assert!(
                needs_approval(Approve::Send, &send, &strings(&["push", flagged]), false),
                "{flagged}"
            );
        }
    }

    #[test]
    fn the_command_line_a_person_reads() {
        let line = command_line(
            "/usr/bin/deploy",
            &strings(&["--key-file", "{keyfile}"]),
            &strings(&["push", "it's", "a b", ""]),
        );
        assert_eq!(
            line,
            r"/usr/bin/deploy --key-file {keyfile} push 'it'\''s' 'a b' ''"
        );
    }

    #[test]
    fn output_is_cut_at_a_character_and_says_how_much() {
        assert_eq!(shown_output("short", 5), "short");
        // An odd start, so the cut falls inside a two-byte character (review round 1, slice 2).
        let long = format!("x{}", "é".repeat(OUTPUT_SHOWN));
        let shown = shown_output(&long, long.len() as u64);
        assert!(shown.len() < OUTPUT_SHOWN + 40, "{}", shown.len());
        assert!(shown.starts_with("xé"));
        assert!(
            shown.ends_with(&format!(
                "[… {} bytes more]",
                long.len() - (OUTPUT_SHOWN - 1)
            )),
            "{}",
            &shown[shown.len() - 40..]
        );
    }

    #[test]
    fn shown_output_fits_a_json_answer() {
        // Quotes take two bytes each once written as JSON: half as many fit.
        let quotes = "\"".repeat(OUTPUT_SHOWN);
        let shown = shown_output(&quotes, quotes.len() as u64);
        assert_eq!(shown.matches('"').count(), OUTPUT_SHOWN / 2);
        // What was read back is not all the stream: the whole is counted.
        assert_eq!(shown_output("abc", 10), "abc\n[… 7 bytes more]");
    }

    #[test]
    fn a_refusal_names_the_argument_by_place_never_by_value() {
        let value = "sk-live-0123456789abcdef/xyz";
        for (args, refuse) in [
            (vec![value.to_owned()], Vec::new()),
            (vec![format!("--token={value}")], strings(&["--token"])),
            (vec!["a\u{1}b".to_owned()], Vec::new()),
        ] {
            let reason = args_refusal(&args, &refuse, &[], false).unwrap_or_default();
            assert!(reason.contains("argument 1"), "{reason}");
            assert!(!reason.contains("sk-live"), "{reason}");
        }
        assert!(
            args_refusal(
                &strings(&["x"; 2])
                    .iter()
                    .map(|_| "y".repeat(4000))
                    .collect::<Vec<_>>(),
                &[],
                &[],
                false
            )
            .is_none()
        );
        let big: Vec<String> = (0..20).map(|_| "y".repeat(4000)).collect();
        assert!(args_refusal(&big, &[], &[], false).is_some_and(|r| r.contains("KiB in all")));
    }
}
