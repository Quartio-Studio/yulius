//! Task spec template and new-project scaffolds (SPEC-001 PRJ-004, PRJ-005, PRJ-008,
//! DSC-001..006).
//!
//! Every scaffold contains the enforced format (`AGENTS.md`, `CLAUDE.md`, `yulius.toml`,
//! product, architecture with Mermaid, ADR 0001, changes, specs directory) and, except for the
//! empty template, a minimal runnable skeleton without third-party dependencies plus a test
//! command. The empty template leaves technology and commands to the first task, which must
//! decide them and record an ADR (DSC-006).

use crate::cron::CivilDate;
use crate::ids::TaskId;

use super::{Commands, ProjectConfig, render_yulius_toml, slug_or};

/// Project templates offered at creation (PRJ-008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TemplateKind {
    /// Yulius format only; no code.
    Empty,
    /// Node.js web application using only the standard library and `node:test`.
    WebApplication,
    /// Python data project using only the standard library and `unittest`.
    PythonData,
    /// Rust HTTP service using only `std`.
    RustService,
}

impl TemplateKind {
    /// All templates in display order.
    pub const ALL: [Self; 4] = [
        Self::Empty,
        Self::WebApplication,
        Self::PythonData,
        Self::RustService,
    ];

    /// Stable identifier.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::WebApplication => "web",
            Self::PythonData => "python-data",
            Self::RustService => "rust-service",
        }
    }

    /// Parses an identifier.
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == id)
    }

    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Empty => "Empty project (Yulius format only)",
            Self::WebApplication => "Web application",
            Self::PythonData => "Python data project",
            Self::RustService => "Rust service",
        }
    }

    /// One-line description.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Empty => {
                "Documentation format and rules only; the first task chooses the technology."
            }
            Self::WebApplication => {
                "Node.js HTTP server with a testable request handler; no dependencies."
            }
            Self::PythonData => {
                "Python package summarizing CSV data with unittest tests; standard library only."
            }
            Self::RustService => "Rust HTTP service on std::net with unit tests; no dependencies.",
        }
    }
}

fn single_line(text: &str, max_chars: usize) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect()
}

/// A new task spec with the required frontmatter and sections; placeholders are HTML comments,
/// so the spec stays invalid until the agent fills every section and the Plan checklist.
#[must_use]
pub fn spec_template(title: &str, task_id: &TaskId, date: CivilDate) -> String {
    let title = match single_line(title, 200) {
        empty if empty.is_empty() => "Untitled task".to_owned(),
        title => title,
    };
    format!(
        "---\n\
         yulius-task: {task_id}\n\
         status: draft\n\
         ---\n\
         \n\
         # {title}\n\
         \n\
         Date: {date}\n\
         \n\
         ## Goal\n\
         \n\
         <!-- The outcome this task delivers and why, in one or two sentences. -->\n\
         \n\
         ## Context\n\
         \n\
         <!-- What exists today, constraints, links to related specs, ADRs and code. -->\n\
         \n\
         ## Requirements\n\
         \n\
         <!-- Observable behavior that must hold when the task is done. -->\n\
         \n\
         ## Plan\n\
         \n\
         <!-- A checklist ('- [ ] step'), kept in sync with Yulius steps (steps_set, step_update). -->\n\
         \n\
         ## Acceptance criteria\n\
         \n\
         <!-- How a person verifies the result. -->\n\
         \n\
         ## Test plan\n\
         \n\
         <!-- Automated tests to add or run, including failure cases. -->\n\
         \n\
         ## Decisions\n\
         \n\
         <!-- Technology and design choices with reasons; record lasting ones as ADRs. -->\n\
         \n\
         ## Evidence\n\
         \n\
         <!-- Required before 'status: done': commands run and their results. -->\n"
    )
}

const RUST_RESERVED: [&str; 12] = [
    "test",
    "std",
    "core",
    "alloc",
    "proc-macro",
    "self",
    "super",
    "crate",
    "fn",
    "type",
    "mod",
    "main",
];
const PYTHON_RESERVED: [&str; 43] = [
    "and",
    "as",
    "assert",
    "async",
    "await",
    "break",
    "class",
    "continue",
    "def",
    "del",
    "elif",
    "else",
    "except",
    "finally",
    "for",
    "from",
    "global",
    "if",
    "import",
    "in",
    "is",
    "lambda",
    "nonlocal",
    "not",
    "or",
    "pass",
    "raise",
    "return",
    "try",
    "while",
    "with",
    "yield",
    "match",
    "case",
    "type",
    "csv",
    "statistics",
    "sys",
    "pathlib",
    "unittest",
    "test",
    "tests",
    "data",
];

struct Names {
    display: String,
    package: String,
    rust_crate: String,
    rust_lib: String,
    python: String,
}

fn names(project_name: &str) -> Names {
    let display = match single_line(project_name, 100) {
        empty if empty.is_empty() => "New project".to_owned(),
        name => name,
    };
    let package = slug_or(&display, "app");
    let mut rust_crate = if package.starts_with(|c: char| c.is_ascii_digit()) {
        format!("app-{package}")
    } else {
        package.clone()
    };
    if RUST_RESERVED.contains(&rust_crate.as_str()) {
        rust_crate.push_str("-service");
    }
    let rust_lib = rust_crate.replace('-', "_");
    let mut python = package.replace('-', "_");
    if python.starts_with(|c: char| c.is_ascii_digit()) {
        python.insert_str(0, "p_");
    }
    if PYTHON_RESERVED.contains(&python.as_str()) {
        python.push_str("_project");
    }
    Names {
        display,
        package,
        rust_crate,
        rust_lib,
        python,
    }
}

fn commands(template: TemplateKind, names: &Names) -> Commands {
    let some = |text: &str| Some(text.to_owned());
    match template {
        TemplateKind::Empty => Commands::default(),
        TemplateKind::WebApplication => Commands {
            test: some("node --test"),
            lint: None,
            build: None,
        },
        TemplateKind::PythonData => Commands {
            test: some("python3 -m unittest discover -s tests"),
            lint: Some(format!("python3 -m compileall -q {} tests", names.python)),
            build: None,
        },
        TemplateKind::RustService => Commands {
            test: some("cargo test"),
            lint: some("cargo clippy --all-targets -- -D warnings"),
            build: some("cargo build --release"),
        },
    }
}

fn run_command(template: TemplateKind, names: &Names) -> Option<String> {
    match template {
        TemplateKind::Empty => None,
        TemplateKind::WebApplication => Some("node src/server.js".to_owned()),
        TemplateKind::PythonData => {
            Some(format!("python3 -m {} data/sample.csv value", names.python))
        }
        TemplateKind::RustService => Some("cargo run".to_owned()),
    }
}

fn layout_lines(template: TemplateKind) -> &'static str {
    match template {
        TemplateKind::Empty => "",
        TemplateKind::WebApplication => {
            "- `src/app.js`: request handling, free of networking so it is easy to test.\n\
             - `src/server.js`: HTTP server; listens on `PORT` (default 3000) and `HOST`.\n\
             - `test/`: `node:test` tests.\n"
        }
        TemplateKind::PythonData => {
            "- `<package>/stats.py`: loading and summarizing CSV columns.\n\
             - `<package>/__main__.py`: command-line entry point.\n\
             - `data/sample.csv`: sample input.\n\
             - `tests/`: `unittest` tests.\n"
        }
        TemplateKind::RustService => {
            "- `src/lib.rs`: request routing and response rendering, unit-tested.\n\
             - `src/main.rs`: TCP listener; listens on `PORT` (default 8080).\n"
        }
    }
}

fn agents_md(template: TemplateKind, names: &Names, commands: &Commands) -> String {
    let name = &names.display;
    let mut rows = Vec::new();
    for (purpose, command) in [
        ("Test", commands.test.clone()),
        ("Lint", commands.lint.clone()),
        ("Build", commands.build.clone()),
        ("Run", run_command(template, names)),
    ] {
        if let Some(command) = command {
            rows.push(format!("| {purpose} | `{command}` |"));
        }
    }
    let commands_section = if rows.is_empty() {
        "No technology is chosen yet. The first task decides it (record an ADR), then adds the\n\
         test, lint and build commands here and in `yulius.toml` `[commands]`.\n"
            .to_owned()
    } else {
        format!(
            "| Purpose | Command |\n| --- | --- |\n{}\n",
            rows.join("\n")
        )
    };
    let layout = layout_lines(template).replace("<package>", &names.python);
    format!(
        "# {name}\n\
         \n\
         Entry point for coding agents working on {name}. `CLAUDE.md` points here. The project\n\
         summary is [docs/product.md](docs/product.md).\n\
         \n\
         ## Rules\n\
         \n\
         1. Know the product first: read the docs index below, earlier specs, parallel and\n   \
            previous tasks and earlier sessions before designing; extend what exists.\n\
         2. Features and fixes need a spec; research, questions, docs and chores do not. A spec\n   \
            is `docs/specs/YYYY-MM-DD-<slug>.md` with frontmatter (`yulius-task`, `status`) and\n   \
            the sections Goal, Context, Requirements, Plan (a `- [ ]` checklist), Acceptance\n   \
            criteria, Test plan and Decisions. When the task is complete, check every Plan item,\n   \
            add Evidence and set `status: done`.\n\
         3. Steps measure real progress: 3-12 steps of similar effort, each a slice that leaves\n   \
            the project working, finished one at a time as the work happens. Keep the spec's\n   \
            Plan and the Yulius steps in sync (`steps_set`, `step_update`).\n\
         4. Tests prove behavior: a test that fails without the change, then the change; strong\n   \
            unit and integration tests; wait on conditions, never on sleeps; no retries of\n   \
            flaky tests.\n\
         5. Make technology and design decisions yourself: compare options, choose, justify,\n   \
            and record each lasting decision as an ADR in `docs/decisions/NNNN-<slug>.md`\n   \
            (Status, Context, Decision, Alternatives, Consequences).\n\
         6. Keep `docs/architecture.md` (with its Mermaid diagram), `docs/product.md` and this\n   \
            file true in the same change. Production-grade defaults: explicit error handling,\n   \
            no secrets in the repository.\n\
         7. On completion add a short entry at the top of `docs/changes.md`.\n\
         \n\
         ## Delivery\n\
         \n\
         Every feature and fix takes the same autonomous path: spec, implementation slice by\n\
         slice, independent review in a fresh session, fixes, gates, a small pull request into\n\
         `dev`, merge on green, deploy and verify `dev`, promotion to `main` by a merge commit,\n\
         verify production. There is no human acceptance step; the gates decide.\n\
         \n\
         - One task, one branch, one worktree from `dev`; never commit or push to `dev` or\n  \
           `main` directly, never `--no-verify`, no `git stash` as a tool, force-push only your\n  \
           own branch with `--force-with-lease`.\n\
         - Small atomic commits with conventional messages; pull requests of one concern, about\n  \
           400 changed lines outside tests. Features are squash-merged into `dev`.\n\
         - CI runs on our own machines through Yulius CI (the `gate` command in `yulius.toml`);\n  \
           never add GitHub Actions or paid GitHub features.\n\
         - After the merge, delete the branch and the worktree; leave nothing stale.\n\
         \n\
         ## Commands\n\
         \n\
         {commands_section}\
         \n\
         Development servers must use the port range Yulius assigns to this project (see the\n\
         session briefing).\n\
         \n\
         ## Layout (the docs index)\n\
         \n\
         - `docs/product.md`: what the project is for, users, scope, current state.\n\
         - `docs/architecture.md`: components and data flow (Mermaid).\n\
         - `docs/decisions/`: architecture decision records.\n\
         - `docs/specs/`: one spec per task.\n\
         - `docs/changes.md`: delivered changes, newest first.\n\
         {layout}"
    )
}

const CLAUDE_MD: &str = "# Claude Code\n\nRead [AGENTS.md](AGENTS.md) first: it is the canonical project context and rules.\n";

fn product_md(template: TemplateKind, names: &Names) -> String {
    format!(
        "# {}\n\
         \n\
         ## Purpose\n\
         \n\
         To be described by the first task: what the project is for and the problem it solves.\n\
         \n\
         ## Users\n\
         \n\
         To be described: who uses it and what they need.\n\
         \n\
         ## Scope\n\
         \n\
         To be described: what is in and out of scope.\n\
         \n\
         ## Current state\n\
         \n\
         Created by Yulius from the \"{}\" template. {}\n",
        names.display,
        template.name(),
        template.description()
    )
}

fn architecture_md(template: TemplateKind, names: &Names) -> String {
    let (summary, diagram) = match template {
        TemplateKind::Empty => (
            "No technology has been chosen yet. Replace this diagram when the first task decides\n\
             the architecture (and records the decision as an ADR).",
            "flowchart LR\n  User[\"User\"] --> System[\"System (technology to be decided)\"]",
        ),
        TemplateKind::WebApplication => (
            "A Node.js HTTP server delegates every request to a pure handler function, which\n\
             keeps routing testable without opening sockets.",
            "flowchart LR\n  Browser[\"Browser\"] -->|HTTP| Server[\"src/server.js\"]\n  Server --> App[\"src/app.js handle()\"]\n  Tests[\"test/*.test.js\"] --> App",
        ),
        TemplateKind::PythonData => (
            "A small Python package loads numeric CSV columns and summarizes them; the command\n\
             line entry point and the tests use the same functions.",
            "flowchart LR\n  CSV[\"data/*.csv\"] --> Load[\"load_values()\"]\n  Load --> Summary[\"summarize()\"]\n  Summary --> CLI[\"python -m package\"]\n  Tests[\"tests/\"] --> Summary",
        ),
        TemplateKind::RustService => (
            "A std-only TCP listener reads the request line and delegates routing to a pure\n\
             library function that is unit-tested.",
            "flowchart LR\n  Client[\"HTTP client\"] -->|TCP| Main[\"src/main.rs\"]\n  Main --> Lib[\"src/lib.rs handle()\"]\n  Tests[\"unit tests\"] --> Lib",
        ),
    };
    format!(
        "# Architecture of {}\n\n{summary}\n\n```mermaid\n{diagram}\n```\n",
        names.display
    )
}

const ADR_0001: &str = "# 1. Record architecture decisions\n\
\n\
## Status\n\
\n\
Accepted\n\
\n\
## Context\n\
\n\
Coding agents work on this project across many sessions. Without a durable record, later\n\
sessions cannot see why earlier choices were made and decisions drift.\n\
\n\
## Decision\n\
\n\
Record every lasting technology or design decision as an architecture decision record in\n\
`docs/decisions/NNNN-<slug>.md` with the sections Status, Context, Decision, Alternatives and\n\
Consequences. A superseded record keeps its file and links to its replacement.\n\
\n\
## Alternatives\n\
\n\
- Decisions only in commit messages or chat: not discoverable by later sessions.\n\
- One growing decisions file: harder to reference, review and supersede.\n\
\n\
## Consequences\n\
\n\
Every task that makes a lasting decision adds or supersedes a record. The Yulius briefing lists\n\
record titles so new sessions stay consistent with earlier decisions.\n";

fn changes_md(template: TemplateKind) -> String {
    format!(
        "# Changes\n\
         \n\
         Delivered changes, newest first. Add an entry at the top when a task is completed.\n\
         \n\
         ## Project created\n\
         \n\
         - Scaffolded by Yulius from the \"{}\" template.\n",
        template.name()
    )
}

const SPECS_README: &str = "# Task specs\n\nOne spec per task, named `YYYY-MM-DD-<slug>.md`. See the rules in [AGENTS.md](../../AGENTS.md).\n";

fn web_files(names: &Names) -> Vec<(String, String)> {
    let package_json = format!(
        "{{\n  \"name\": \"{}\",\n  \"version\": \"0.1.0\",\n  \"private\": true,\n  \"type\": \"module\",\n  \"scripts\": {{\n    \"start\": \"node src/server.js\",\n    \"test\": \"node --test\"\n  }},\n  \"engines\": {{\n    \"node\": \">=20\"\n  }}\n}}\n",
        names.package
    );
    let app = r#"// Request handling, kept free of networking so it is easy to test.

const PAGE = `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Hello</title>
  </head>
  <body>
    <h1>Hello from your new web application</h1>
  </body>
</html>
`;

export function handle(method, url) {
  const path = (url ?? "/").split("?")[0];
  if (method === "GET" && path === "/") {
    return { status: 200, type: "text/html; charset=utf-8", body: PAGE };
  }
  if (method === "GET" && path === "/health") {
    return { status: 200, type: "application/json", body: JSON.stringify({ ok: true }) };
  }
  return { status: 404, type: "text/plain; charset=utf-8", body: "Not found\n" };
}
"#;
    let server = r#"import { createServer } from "node:http";
import { handle } from "./app.js";

const port = Number(process.env.PORT ?? 3000);
const host = process.env.HOST ?? "127.0.0.1";

createServer((request, response) => {
  const result = handle(request.method, request.url);
  response.writeHead(result.status, { "content-type": result.type });
  response.end(result.body);
}).listen(port, host, () => {
  console.log(`Listening on http://${host}:${port}`);
});
"#;
    let test = r#"import { test } from "node:test";
import assert from "node:assert/strict";
import { handle } from "../src/app.js";

test("serves the home page", () => {
  const result = handle("GET", "/");
  assert.equal(result.status, 200);
  assert.match(result.body, /<h1>/);
});

test("reports health", () => {
  const result = handle("GET", "/health?probe=1");
  assert.equal(result.status, 200);
  assert.deepEqual(JSON.parse(result.body), { ok: true });
});

test("answers unknown paths with 404", () => {
  assert.equal(handle("GET", "/missing").status, 404);
  assert.equal(handle("POST", "/").status, 404);
});
"#;
    vec![
        ("package.json".to_owned(), package_json),
        ("src/app.js".to_owned(), app.to_owned()),
        ("src/server.js".to_owned(), server.to_owned()),
        ("test/app.test.js".to_owned(), test.to_owned()),
        (".gitignore".to_owned(), "node_modules/\n".to_owned()),
    ]
}

fn python_files(names: &Names) -> Vec<(String, String)> {
    let package = &names.python;
    let pyproject = format!(
        "[project]\nname = \"{}\"\nversion = \"0.1.0\"\nrequires-python = \">=3.10\"\n",
        names.package
    );
    let init = format!(
        "\"\"\"{} data project.\"\"\"\n",
        names.display.replace('"', "'")
    );
    let stats = r#""""Loading and summarizing numeric CSV columns with the standard library."""

import csv
import statistics
from pathlib import Path


def load_values(path, column):
    """Return the numeric values of ``column`` in the CSV file at ``path``, skipping blanks."""
    with Path(path).open(newline="", encoding="utf-8") as handle:
        return [float(row[column]) for row in csv.DictReader(handle) if row[column].strip()]


def summarize(values):
    """Return count, mean, median, min and max of ``values``; raise ValueError when empty."""
    if not values:
        raise ValueError("no values to summarize")
    return {
        "count": len(values),
        "mean": statistics.fmean(values),
        "median": statistics.median(values),
        "min": min(values),
        "max": max(values),
    }
"#;
    let main = format!(
        "\"\"\"Command line: python3 -m {package} [CSV] [COLUMN].\"\"\"\n\nimport sys\n\nfrom .stats import load_values, summarize\n\n\ndef main(argv):\n    path = argv[1] if len(argv) > 1 else \"data/sample.csv\"\n    column = argv[2] if len(argv) > 2 else \"value\"\n    print(summarize(load_values(path, column)))\n    return 0\n\n\nif __name__ == \"__main__\":\n    sys.exit(main(sys.argv))\n"
    );
    let test = format!(
        "import unittest\nfrom pathlib import Path\n\nfrom {package}.stats import load_values, summarize\n\nSAMPLE = Path(__file__).resolve().parent.parent / \"data\" / \"sample.csv\"\n\n\nclass SummarizeTest(unittest.TestCase):\n    def test_summary_of_sample_data(self):\n        summary = summarize(load_values(SAMPLE, \"value\"))\n        self.assertEqual(summary[\"count\"], 3)\n        self.assertEqual(summary[\"mean\"], 4.0)\n        self.assertEqual(summary[\"median\"], 4.0)\n        self.assertEqual((summary[\"min\"], summary[\"max\"]), (3.0, 5.0))\n\n    def test_empty_input_is_rejected(self):\n        with self.assertRaises(ValueError):\n            summarize([])\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n"
    );
    vec![
        ("pyproject.toml".to_owned(), pyproject),
        (format!("{package}/__init__.py"), init),
        (format!("{package}/stats.py"), stats.to_owned()),
        (format!("{package}/__main__.py"), main),
        (
            "data/sample.csv".to_owned(),
            "day,value\n2026-01-01,3\n2026-01-02,5\n2026-01-03,4\n".to_owned(),
        ),
        ("tests/test_stats.py".to_owned(), test),
        (".gitignore".to_owned(), "__pycache__/\n.venv/\n".to_owned()),
    ]
}

fn rust_files(names: &Names) -> Vec<(String, String)> {
    let cargo = format!(
        "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n[dependencies]\n",
        names.rust_crate
    );
    let lib = r#"//! Request routing and response rendering, free of networking so it is easy to test.

/// An HTTP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// Status code and reason phrase, for example `200 OK`.
    pub status: &'static str,
    /// `Content-Type` header value.
    pub content_type: &'static str,
    /// Response body.
    pub body: String,
}

/// Routes an HTTP request line such as `GET /health HTTP/1.1`.
pub fn handle(request_line: &str) -> Response {
    let mut parts = request_line.split_whitespace();
    let method = parts.next();
    let path = parts.next().map(|target| target.split('?').next().unwrap_or(target));
    match (method, path) {
        (Some("GET"), Some("/")) => Response {
            status: "200 OK",
            content_type: "text/plain; charset=utf-8",
            body: "Hello from your new Rust service\n".to_owned(),
        },
        (Some("GET"), Some("/health")) => Response {
            status: "200 OK",
            content_type: "application/json",
            body: "{\"ok\":true}".to_owned(),
        },
        _ => Response {
            status: "404 Not Found",
            content_type: "text/plain; charset=utf-8",
            body: "Not found\n".to_owned(),
        },
    }
}

/// Serializes a response as an HTTP/1.1 message.
pub fn render(response: &Response) -> String {
    format!(
        "HTTP/1.1 {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        response.status,
        response.content_type,
        response.body.len(),
        response.body
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_home_and_health() {
        assert_eq!(handle("GET / HTTP/1.1").status, "200 OK");
        let health = handle("GET /health?probe=1 HTTP/1.1");
        assert_eq!(health.body, "{\"ok\":true}");
    }

    #[test]
    fn unknown_requests_are_404() {
        assert_eq!(handle("GET /missing HTTP/1.1").status, "404 Not Found");
        assert_eq!(handle("POST / HTTP/1.1").status, "404 Not Found");
        assert_eq!(handle("").status, "404 Not Found");
    }

    #[test]
    fn renders_content_length() {
        let text = render(&handle("GET /health HTTP/1.1"));
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("content-length: 11\r\n"));
    }
}
"#;
    let main = r#"use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

/// Longest request line read from a client.
const MAX_REQUEST_LINE: u64 = 8 * 1024;

fn serve(mut stream: TcpStream) -> std::io::Result<()> {
    let mut request_line = String::new();
    BufReader::new((&stream).take(MAX_REQUEST_LINE)).read_line(&mut request_line)?;
    let response = __LIB__::handle(&request_line);
    stream.write_all(__LIB__::render(&response).as_bytes())
}

fn main() -> std::io::Result<()> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_owned());
    let listener = TcpListener::bind(format!("127.0.0.1:{port}"))?;
    println!("Listening on http://127.0.0.1:{port}");
    for stream in listener.incoming() {
        if let Err(error) = stream.and_then(serve) {
            eprintln!("request failed: {error}");
        }
    }
    Ok(())
}
"#
    .replace("__LIB__", &names.rust_lib);
    vec![
        ("Cargo.toml".to_owned(), cargo),
        ("src/lib.rs".to_owned(), lib.to_owned()),
        ("src/main.rs".to_owned(), main),
        (".gitignore".to_owned(), "/target\n".to_owned()),
    ]
}

/// The files of a new project: the enforced format plus the template skeleton, as
/// `(repository path, contents)` in a stable order. The result passes
/// [`super::validate_project`].
#[must_use]
pub fn scaffold_files(project_name: &str, template: TemplateKind) -> Vec<(String, String)> {
    let names = names(project_name);
    let config = ProjectConfig {
        commands: commands(template, &names),
        ..ProjectConfig::default()
    };
    let mut yulius_toml = render_yulius_toml(&config);
    if template == TemplateKind::Empty {
        yulius_toml = yulius_toml.replace(
            "[commands]\n",
            "[commands]\n# Set test, lint and build once the first task chooses the technology.\n",
        );
    }
    let mut files = vec![
        (
            "AGENTS.md".to_owned(),
            agents_md(template, &names, &config.commands),
        ),
        ("CLAUDE.md".to_owned(), CLAUDE_MD.to_owned()),
        ("yulius.toml".to_owned(), yulius_toml),
        ("docs/product.md".to_owned(), product_md(template, &names)),
        (
            "docs/architecture.md".to_owned(),
            architecture_md(template, &names),
        ),
        (
            "docs/decisions/0001-record-architecture-decisions.md".to_owned(),
            ADR_0001.to_owned(),
        ),
        ("docs/changes.md".to_owned(), changes_md(template)),
        ("docs/specs/README.md".to_owned(), SPECS_README.to_owned()),
    ];
    files.extend(match template {
        TemplateKind::Empty => Vec::new(),
        TemplateKind::WebApplication => web_files(&names),
        TemplateKind::PythonData => python_files(&names),
        TemplateKind::RustService => rust_files(&names),
    });
    files
}

#[cfg(test)]
mod tests {
    use super::super::{ProjectFiles, parse_yulius_toml, validate_project, validate_spec};
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn file<'a>(files: &'a [(String, String)], path: &str) -> Result<&'a str, String> {
        files
            .iter()
            .find(|(candidate, _)| candidate == path)
            .map(|(_, contents)| contents.as_str())
            .ok_or_else(|| format!("missing {path}"))
    }

    #[test]
    fn every_scaffold_passes_the_project_validator() -> TestResult {
        for template in TemplateKind::ALL {
            let files = scaffold_files("Online Shop", template);
            let report = validate_project(&ProjectFiles::from_files(&files));
            assert!(
                report.is_valid(),
                "{}: {:?}",
                template.id(),
                report.problems
            );
            let config = report.config.ok_or("config")?;
            assert_eq!(config.commands, commands(template, &names("Online Shop")));
            let mut paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
            let total = paths.len();
            paths.sort_unstable();
            paths.dedup();
            assert_eq!(paths.len(), total, "duplicate paths in {}", template.id());
        }
        Ok(())
    }

    #[test]
    fn scaffold_contents_per_template() -> TestResult {
        let web = scaffold_files("Online Shop", TemplateKind::WebApplication);
        assert!(file(&web, "package.json")?.contains("\"name\": \"online-shop\""));
        assert!(file(&web, "AGENTS.md")?.contains("| Test | `node --test` |"));
        let agents = file(&web, "AGENTS.md")?;
        for needle in [
            "never commit or push to `dev` or\n  `main` directly",
            "Features and fixes need a spec; research, questions, docs and chores do not.",
            "3-12 steps of similar effort",
            "independent review in a fresh session",
            "no `git stash` as a tool",
            "never add GitHub Actions",
            "## Layout (the docs index)",
        ] {
            assert!(agents.contains(needle), "AGENTS.md lacks {needle}");
        }
        let python = scaffold_files("Online Shop", TemplateKind::PythonData);
        assert!(file(&python, "online_shop/stats.py").is_ok());
        assert!(file(&python, "tests/test_stats.py")?.contains("from online_shop.stats import"));
        let rust = scaffold_files("3D Model", TemplateKind::RustService);
        assert!(file(&rust, "Cargo.toml")?.contains("name = \"app-3d-model\""));
        assert!(file(&rust, "src/main.rs")?.contains("app_3d_model::handle"));
        let empty = scaffold_files("Empty", TemplateKind::Empty);
        assert_eq!(empty.len(), 8);
        assert!(file(&empty, "AGENTS.md")?.contains("No technology is chosen yet"));
        assert_eq!(
            parse_yulius_toml(file(&empty, "yulius.toml")?)?.commands,
            Commands::default()
        );
        Ok(())
    }

    #[test]
    fn names_are_sanitized() {
        let odd = names("  class \n\t");
        assert_eq!(odd.display, "class");
        assert_eq!(odd.python, "class_project");
        assert_eq!(names("").display, "New project");
        assert_eq!(names("🚀").package, "app");
        assert_eq!(names("Test").rust_crate, "test-service");
        assert_eq!(names("Żółta Łódź").package, "zolta-lodz");
    }

    #[test]
    fn template_ids_round_trip() {
        for template in TemplateKind::ALL {
            assert_eq!(TemplateKind::parse(template.id()), Some(template));
            assert!(!template.name().is_empty() && !template.description().is_empty());
        }
        assert_eq!(TemplateKind::parse("php"), None);
    }

    #[test]
    fn spec_template_is_structured_but_unfilled() -> TestResult {
        let task = TaskId::parse("task-7")?;
        let date = CivilDate::new(2026, 10, 7).ok_or("date")?;
        let text = spec_template("Checkout\nAPI", &task, date);
        assert!(text.starts_with(
            "---\nyulius-task: task-7\nstatus: draft\n---\n\n# Checkout API\n\nDate: 2026-10-07\n"
        ));
        let report = validate_spec(&text);
        assert_eq!(report.task_id.as_deref(), Some("task-7"));
        let problems: Vec<String> = report.problems.iter().map(ToString::to_string).collect();
        assert_eq!(
            problems,
            vec![
                "Section '## Goal' (line 10) is empty.",
                "Section '## Context' (line 14) is empty.",
                "Section '## Requirements' (line 18) is empty.",
                "Section '## Plan' (line 22) has no checklist item; list the steps as '- [ ] step'.",
                "Section '## Acceptance criteria' (line 26) is empty.",
                "Section '## Test plan' (line 30) is empty.",
                "Section '## Decisions' (line 34) is empty.",
            ]
        );
        assert!(spec_template("  ", &task, date).contains("# Untitled task\n"));
        Ok(())
    }
}
