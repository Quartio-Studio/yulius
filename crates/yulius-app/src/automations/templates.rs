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
        // SPEC-251 (#594): GitHub calls an admin outside the organization `CONTRIBUTOR`, and an
        // agent's issue filed on a person's behalf is not by a bot: the repository permission and
        // this project's App decide, never the association alone.
        for wording in [
            "author: .user.login",
            "app: .performed_via_github_app.slug",
            "the author is a person (`bot` is false) with write access",
            "although the association reads anything else",
            "once per author",
            "gh api repos/{owner}/{repo}/collaborators/<login>/permission",
            "`write`, `maintain` or `admin`",
            "an error or any other answer counts as no write access",
            // The App's user token proves no write access: only its bot's issues count by the App.
            "this project's Yulius App's bot (author `<slug>[bot]`) opened it and its body ends with `Opened by task #<n> in Yulius.`",
            "counts as that person's own issue, by the two rules above",
            "gh api graphql -f query='{viewer{login}}'",
            "gh api user/installations",
            "no issue is in scope by this rule",
            "without write access are out of scope",
            // gh returns at most 100 comments per issue.
            "exactly 100 comments has more",
            "`gh issue view <number> --json comments`",
        ] {
            assert!(one_line.contains(wording), "{wording}");
        }
        assert!(!one_line.contains("issues opened by a bot whose body ends"));
        assert!(!one_line.contains("whether the author is its bot or a person"));
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
            "`issues/<number>`",
            "`issue #<number>`",
            "`Part of #<number>`",
            // Every open pull request, not gh's default 30, and every closing keyword.
            "gh pr list --state open --limit 1000 --json number,body",
            "closing keywords, in any case",
            // The comments the drop rules read: open issues only, never the whole history.
            "gh issue list --state open --limit 1000 --json number,comments",
            "whose latest verdict is a `Not actionable yet:` comment",
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

    /// SPEC-240 AGM-003: a run's task is a goal task, and a prompt past the goal's cap is cut, so
    /// each template's first paragraph (what the run does and how it ends) stays in the goal.
    #[test]
    fn every_goal_keeps_how_its_run_ends() {
        use yulius_domain::request::{DoneDefinition, Request, goal_condition};
        let done = DoneDefinition {
            integration_branch: "dev".into(),
            production_branch: "main".into(),
            integration_environments: vec!["dev".into()],
            playground: false,
        };
        let path = format!(
            "/var/lib/yulius/sandboxes/{}/home/.yulius/tasks/999.md",
            "x".repeat(36)
        );
        let one_line = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        for t in templates() {
            let goal = one_line(&goal_condition(&Request::parse(&t.prompt), &path, &done));
            let first = one_line(t.prompt.trim().split("\n\n").next().unwrap_or_default());
            assert!(goal.contains(&first), "{}: {goal}", t.id);
        }
        // Its last step comes after the cut: the goal must say the run ends with its summary.
        let issues = template("open-issues")
            .map(|t| one_line(&t.prompt))
            .unwrap_or_default();
        let first = issues.split(" 1. Collect.").next().unwrap_or_default();
        for wording in [
            "opens no pull request",
            "does not wait for the tasks it starts",
        ] {
            assert!(first.contains(wording), "{wording}");
        }
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
