//! Ready-made automations: the `catalog/automations/*.md` files embedded by `build.rs` (SPEC-042
//! AUT-007..009).
//!
//! They are parsed once, on first use, and never run by themselves: enabling one creates (or
//! re-enables) an ordinary automation with its content. A project's automation with the same
//! name, compared case-insensitively, counts as the template's, so no link is stored. A file that
//! does not parse is skipped with a warning; the unit test below keeps the shipped catalog valid.

use std::sync::OnceLock;

use yulius_domain::automation_format::{AutomationDefinition, parse_automation};

include!(concat!(env!("OUT_DIR"), "/automations.rs"));

static TEMPLATES: OnceLock<Vec<AutomationDefinition>> = OnceLock::new();

/// Parses `files` ((stem, contents)) into list order: `order`, then name. Files that do not
/// parse are skipped with a warning.
#[must_use]
pub fn load(files: &[(&str, &str)]) -> Vec<AutomationDefinition> {
    let mut parsed: Vec<AutomationDefinition> = files
        .iter()
        .filter_map(|(stem, text)| match parse_automation(stem, text) {
            Ok(definition) => Some(definition),
            Err(err) => {
                tracing::warn!(file = %format!("catalog/automations/{stem}.md"), error = %err, "skipping a ready-made automation that does not parse");
                None
            }
        })
        .collect();
    parsed.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    parsed
}

/// The ready-made automations, in list order.
#[must_use]
pub fn templates() -> &'static [AutomationDefinition] {
    TEMPLATES.get_or_init(|| load(AUTOMATION_FILES))
}

/// The ready-made automation with this id, if any.
#[must_use]
pub fn template(id: &str) -> Option<&'static AutomationDefinition> {
    templates().iter().find(|t| t.id == id)
}

/// Whether an automation named `name` is the template's (case-insensitive, trimmed).
#[must_use]
pub fn same_name(template: &AutomationDefinition, name: &str) -> bool {
    template.name.trim().to_lowercase() == name.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shipped file parses, the build script embedded exactly the directory's files, and
    /// the two ready-made automations of SPEC-042 run daily at 03:00 CEST (UTC+2).
    #[test]
    fn the_shipped_catalog_is_valid() -> Result<(), Box<dyn std::error::Error>> {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../catalog/automations");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.ends_with(".md") && n != "README.md")
            .map(|n| n.trim_end_matches(".md").to_owned())
            .collect();
        on_disk.sort();
        let embedded: Vec<&str> = AUTOMATION_FILES.iter().map(|(s, _)| *s).collect();
        assert_eq!(embedded, on_disk, "build.rs embeds every catalog file");
        for (stem, text) in AUTOMATION_FILES {
            if let Err(err) = parse_automation(stem, text) {
                return Err(format!("catalog/automations/{stem}.md {err}").into());
            }
        }
        let ids: Vec<&str> = templates().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            ["docs-up-to-date", "open-issues", "nightly-dev-deploy"]
        );
        for t in templates() {
            assert_eq!((t.cron.as_str(), t.utc_offset_minutes), ("0 3 * * *", 120));
            assert!(
                t.prompt.len() <= crate::lifecycle::MAX_PROMPT_BYTES,
                "{}",
                t.id
            );
        }
        let docs = template("docs-up-to-date").ok_or("docs template")?;
        assert_eq!(docs.workflow.as_deref(), Some("wf-docs"));
        for t in templates() {
            if let Some(workflow) = &t.workflow {
                assert!(
                    crate::workflows::catalog::default_workflow(workflow).is_some(),
                    "{}: unknown workflow {workflow}",
                    t.id
                );
            }
        }
        assert!(docs.prompt.contains("`issue_create`"));
        // Its previous run is found in a busy project too; without one it reviews everything,
        // since `automations_list` shows only the latest run (it cannot tell a first run).
        assert!(docs.prompt.contains("`limit` 200"));
        assert!(!docs.prompt.contains("automations_list"));
        let issues = template("open-issues").ok_or("issues template")?;
        // Wording is matched on one line, so a reflowed prompt keeps it.
        let one_line = issues
            .prompt
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(issues.prompt.contains("Never create a new issue"));
        for association in ["OWNER", "MEMBER", "COLLABORATOR"] {
            assert!(issues.prompt.contains(association), "{association}");
        }
        assert!(issues.prompt.contains("`task_create`"));
        // Issues Yulius's agents opened through the App count as the team's (agent_delivery.rs).
        assert!(issues.prompt.contains("Opened by task #"));
        // Outsiders' comments are data, never instructions for the tasks it starts.
        assert!(issues.prompt.contains("untrusted"));
        // Its own and its tasks' comments do not make an abandoned issue new again.
        assert!(issues.prompt.contains("Needs a person:"));
        assert!(one_line.contains("not this automation or its tasks"));
        // SPEC-194: only the 5 most important actionable issues no running task owns, ranked
        // before any is verified; the rest wait untouched for a later run.
        assert_eq!(
            issues.description,
            "Every night, the core team's open GitHub issues are ranked and the 5 most important actionable ones get a task each."
        );
        for wording in [
            "5 most important actionable",
            "no running task owns",
            // Running tasks, also those paused for capacity, from a busy project's whole list.
            "`tasks_list` (scope `active`, `limit` 200",
            "waiting_capacity",
            // An issue is named by its own reference, never by a task number.
            "title ends with `(#<number>)`",
            "`task #<number>`",
            "`task_get`",
            "never count toward the 5",
            "`Closes`",
            // A high-ranked issue that needs a person is not re-verified every night.
            "`Not actionable yet:`",
            "Rank the rest",
            "`priority: high`",
            "reactions: .reactions.total_count, created_at",
            "`gh issue view <number> --json body,comments`",
            "in rank order",
            "only as many as there are free places",
            "stay untouched: no verification, no comment",
        ] {
            assert!(one_line.contains(wording), "{wording}");
        }
        assert!(!issues.prompt.contains("verify each one"));
        assert!(!issues.prompt.contains("Give each remaining issue"));
        // SPEC-134 CDK-003: dev is deployed and tested every night, never production.
        let deploy = template("nightly-dev-deploy").ok_or("deploy template")?;
        for wording in [
            "`cd_deploy`",
            "`yulius wait`",
            "`cd_status`",
            "`issue_create`",
            "Never deploy production",
            "not defined",
            "empty",
        ] {
            assert!(deploy.prompt.contains(wording), "{wording}");
        }
        Ok(())
    }

    #[test]
    fn order_then_name_bad_files_skipped_and_names_match_loosely() {
        let file = |name: &str, order: &str| {
            format!("---\nname: {name}\ndescription: d\ncron: 0 3 * * *\n{order}---\nPrompt\n")
        };
        let (b, a, late) = (
            file("Beta", "order: 5\n"),
            file("alpha", "order: 5\n"),
            file("Late", ""),
        );
        let loaded = load(&[("late", &late), ("b", &b), ("bad", "nope"), ("a", &a)]);
        let ids: Vec<&str> = loaded.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "late"]);
        assert!(same_name(&loaded[0], "  ALPHA "));
        assert!(!same_name(&loaded[0], "alpha 2"));
    }
}
