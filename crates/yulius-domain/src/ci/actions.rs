//! Reading GitHub Actions workflows for onboarding (F 2026-10-10: "onboarding should contain
//! deleting GitHub Actions completely"): the commands of each job, so an agent can move them
//! into `[[ci.jobs]]` before it deletes `.github/workflows/` and nothing is lost.
//!
//! This is a bounded, line-based reader of the YAML subset workflows use (block mappings,
//! `- ` step lists, `run: |` / `run: >` block scalars and single-line values), not a YAML
//! parser: anything it does not understand is kept as raw text for the agent to read. It never
//! evaluates expressions; `${{ … }}` stays literal.

/// Largest workflow file read.
pub const MAX_WORKFLOW_BYTES: usize = 64 * 1024;
/// Most jobs listed per workflow.
pub const MAX_WORKFLOW_JOBS: usize = 32;
/// Most steps listed per job.
pub const MAX_JOB_STEPS: usize = 64;
/// Longest `run` script kept per step.
pub const MAX_RUN_BYTES: usize = 4 * 1024;
/// Longest raw `strategy` block kept per job.
pub const MAX_STRATEGY_BYTES: usize = 1024;

/// One step of a job.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkflowStep {
    /// `name:`.
    pub name: Option<String>,
    /// `run:` script (block scalars joined with newlines, dedented).
    pub run: Option<String>,
    /// `uses:` action reference (its effect has to be reproduced by hand: a tool installed on
    /// the VM, a cache directory, a setup command).
    pub uses: Option<String>,
}

/// One job of a workflow.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkflowJob {
    /// Job id.
    pub id: String,
    /// `runs-on:` as written.
    pub runs_on: Option<String>,
    /// The raw `strategy:` block (matrix), when present.
    pub strategy: Option<String>,
    /// Steps in order.
    pub steps: Vec<WorkflowStep>,
}

/// A summarized workflow file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkflowSummary {
    /// `name:` at the top level.
    pub name: Option<String>,
    /// Jobs in file order.
    pub jobs: Vec<WorkflowJob>,
    /// The file or a list was cut at a bound.
    pub truncated: bool,
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// `key: value` of a trimmed line (value trimmed, quotes removed, comment dropped).
fn key_value(text: &str) -> Option<(&str, String)> {
    let (key, rest) = text.split_once(':')?;
    let key = key.trim();
    if key.is_empty() || key.contains(' ') && !key.starts_with('"') {
        return None;
    }
    let mut value = rest.trim().to_owned();
    if !(value.starts_with('"') || value.starts_with('\''))
        && let Some(at) = value.find(" #")
    {
        value.truncate(at);
        value = value.trim_end().to_owned();
    }
    let unquoted = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .map(str::to_owned);
    Some((key.trim_matches('"'), unquoted.unwrap_or(value)))
}

fn clip_bytes(mut text: String, max: usize, truncated: &mut bool) -> String {
    if text.len() > max {
        let mut cut = max;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        *truncated = true;
    }
    text
}

/// Lines of the block that follows line `start` (more indented than `parent`), dedented, with
/// blank lines kept; returns them and the index after the block.
fn block(lines: &[&str], start: usize, parent: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut base: Option<usize> = None;
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            out.push(String::new());
            i += 1;
            continue;
        }
        let level = indent(line);
        if level <= parent {
            break;
        }
        let base = *base.get_or_insert(level);
        out.push(line.get(base.min(level)..).unwrap_or("").to_owned());
        i += 1;
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    (out, i)
}

/// Summarizes one workflow file (bounded; never fails: unknown shapes are skipped).
#[must_use]
pub fn summarize_workflow(text: &str) -> WorkflowSummary {
    let mut summary = WorkflowSummary::default();
    let text = if text.len() > MAX_WORKFLOW_BYTES {
        summary.truncated = true;
        let mut cut = MAX_WORKFLOW_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        &text[..cut]
    } else {
        text
    };
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if indent(line) == 0
            && let Some((key, value)) = key_value(line.trim())
        {
            if key == "name" && !value.is_empty() {
                summary.name = Some(value);
            } else if key == "jobs" {
                let (jobs, next) = jobs(&lines, i + 1, &mut summary.truncated);
                summary.jobs = jobs;
                i = next;
                continue;
            }
        }
        i += 1;
    }
    summary
}

fn jobs(lines: &[&str], start: usize, truncated: &mut bool) -> (Vec<WorkflowJob>, usize) {
    let mut out: Vec<WorkflowJob> = Vec::new();
    let mut job_indent: Option<usize> = None;
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        let level = indent(line);
        if level == 0 {
            break;
        }
        let job_level = *job_indent.get_or_insert(level);
        if level == job_level
            && let Some((id, rest)) = key_value(line.trim())
            && rest.is_empty()
        {
            let (body, next) = block(lines, i + 1, level);
            if out.len() < MAX_WORKFLOW_JOBS {
                out.push(job(id, &body, truncated));
            } else {
                *truncated = true;
            }
            i = next;
            continue;
        }
        i += 1;
    }
    (out, i)
}

fn job(id: &str, body: &[String], truncated: &mut bool) -> WorkflowJob {
    let lines: Vec<&str> = body.iter().map(String::as_str).collect();
    let mut job = WorkflowJob {
        id: id.to_owned(),
        ..WorkflowJob::default()
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if indent(line) != 0 || line.trim().is_empty() {
            i += 1;
            continue;
        }
        match key_value(line.trim()) {
            Some(("runs-on", value)) if !value.is_empty() => job.runs_on = Some(value),
            Some(("strategy", _)) => {
                let (raw, next) = block(&lines, i + 1, 0);
                job.strategy = Some(clip_bytes(raw.join("\n"), MAX_STRATEGY_BYTES, truncated));
                i = next;
                continue;
            }
            Some(("steps", _)) => {
                let (raw, next) = block(&lines, i + 1, 0);
                let raw: Vec<&str> = raw.iter().map(String::as_str).collect();
                job.steps = steps(&raw, truncated);
                i = next;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    job
}

fn steps(lines: &[&str], truncated: &mut bool) -> Vec<WorkflowStep> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let level = indent(line);
        let Some(first) = line.trim_start().strip_prefix("- ") else {
            i += 1;
            continue;
        };
        // The step's own keys: the rest of the "- " line, then lines indented past the dash.
        let key_indent = level + 2;
        let mut keys: Vec<String> = vec![format!("{}{first}", " ".repeat(key_indent))];
        let (rest, next) = block(lines, i + 1, level);
        keys.extend(rest.into_iter().map(|l| {
            if l.is_empty() {
                l
            } else {
                format!("{}{l}", " ".repeat(key_indent))
            }
        }));
        if out.len() < MAX_JOB_STEPS {
            out.push(step(&keys, key_indent, truncated));
        } else {
            *truncated = true;
        }
        i = next;
    }
    out
}

fn step(lines: &[String], key_indent: usize, truncated: &mut bool) -> WorkflowStep {
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut step = WorkflowStep::default();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() || indent(line) != key_indent {
            i += 1;
            continue;
        }
        match key_value(line.trim()) {
            Some(("name", value)) => step.name = Some(value),
            Some(("uses", value)) => step.uses = Some(value),
            Some(("run", value)) => {
                if value.starts_with('|') || value.starts_with('>') {
                    let folded = value.starts_with('>');
                    let (body, next) = block(&lines, i + 1, key_indent);
                    let script = if folded {
                        body.join(" ")
                    } else {
                        body.join("\n")
                    };
                    step.run = Some(clip_bytes(script, MAX_RUN_BYTES, truncated));
                    i = next;
                    continue;
                }
                step.run = Some(clip_bytes(value, MAX_RUN_BYTES, truncated));
            }
            _ => {}
        }
        i += 1;
    }
    step
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKFLOW: &str = r#"name: Engineering checks
on:
  pull_request:
  push:
    branches: [dev, main]
jobs:
  quality-linux:
    name: quality (ubuntu-latest)
    runs-on: blacksmith-4vcpu-ubuntu-2404   # runner
    strategy:
      matrix:
        node: [20, 22]
    steps:
      - uses: actions/checkout@abc # v5
        with:
          persist-credentials: false
      - name: Bootstrap
        run: python3 scripts/bootstrap.py
      - name: Gates
        run: |
          scripts/dev ci
          echo "done ${{ github.sha }}"

      - name: Folded
        run: >
          cargo test
          --locked
  policy:
    runs-on: ubuntu-latest
    steps:
      - run: python3 scripts/check_policy.py
"#;

    #[test]
    fn jobs_steps_and_scripts_are_extracted() {
        let summary = summarize_workflow(WORKFLOW);
        assert_eq!(summary.name.as_deref(), Some("Engineering checks"));
        assert!(!summary.truncated);
        let ids: Vec<&str> = summary.jobs.iter().map(|j| j.id.as_str()).collect();
        assert_eq!(ids, ["quality-linux", "policy"]);
        let quality = &summary.jobs[0];
        assert_eq!(
            quality.runs_on.as_deref(),
            Some("blacksmith-4vcpu-ubuntu-2404")
        );
        assert_eq!(quality.strategy.as_deref(), Some("matrix:\n  node: [20, 22]"));
        assert_eq!(quality.steps.len(), 4);
        assert_eq!(quality.steps[0].uses.as_deref(), Some("actions/checkout@abc"));
        assert_eq!(
            quality.steps[1].run.as_deref(),
            Some("python3 scripts/bootstrap.py")
        );
        assert_eq!(
            quality.steps[2].run.as_deref(),
            Some("scripts/dev ci\necho \"done ${{ github.sha }}\"")
        );
        assert_eq!(quality.steps[3].run.as_deref(), Some("cargo test --locked"));
        assert_eq!(
            summary.jobs[1].steps[0].run.as_deref(),
            Some("python3 scripts/check_policy.py")
        );
    }

    #[test]
    fn bounds_and_garbage_never_panic() {
        assert_eq!(summarize_workflow(""), WorkflowSummary::default());
        let garbage = "jobs:\n  - x\n   y: [\n\t:\n- - -\n  a:\n    steps:\n      -\n      - \n";
        let _ = summarize_workflow(garbage);
        let huge = format!("jobs:\n  a:\n    steps:\n{}", "      - run: x\n".repeat(200));
        let summary = summarize_workflow(&huge);
        assert_eq!(summary.jobs[0].steps.len(), MAX_JOB_STEPS);
        assert!(summary.truncated);
        let long = "x".repeat(MAX_WORKFLOW_BYTES + 10);
        assert!(summarize_workflow(&long).truncated);
    }
}
