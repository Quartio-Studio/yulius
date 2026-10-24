//! Default workflows: the `catalog/workflows/*.md` files embedded by `build.rs` (SPEC-002 §7.10
//! "Workflow catalog format", SPEC-001 AUT-006).
//!
//! They are parsed once, on first use, and served read-only as instance workflows with the id
//! `wf-<file stem>`. Nothing is stored: a new release brings new content. A file that does not
//! parse is skipped with a warning; the unit test below keeps the shipped catalog valid.

use std::sync::OnceLock;

use yulius_domain::time::Timestamp;
use yulius_domain::workflow_format::{WorkflowDefinition, parse_workflow};

use super::WorkflowRecord;

include!(concat!(env!("OUT_DIR"), "/workflows.rs"));

/// Prefix of a default workflow's id (`wf-feature` for `feature.md`).
pub const ID_PREFIX: &str = "wf-";

static DEFAULTS: OnceLock<Vec<WorkflowRecord>> = OnceLock::new();

fn record(definition: WorkflowDefinition) -> WorkflowRecord {
    WorkflowRecord {
        id: format!("{ID_PREFIX}{}", definition.id),
        sandbox_id: None,
        name: definition.name,
        description: definition.description,
        steps: definition.steps,
        instructions: definition.instructions,
        require_spec: definition.require_spec,
        require_tests: definition.require_tests,
        created_at: Timestamp(0),
        builtin: true,
        when_to_use: definition.when_to_use,
    }
}

/// Parses `files` ((stem, contents)) into list order: `order`, then name. Files that do not
/// parse are skipped with a warning.
#[must_use]
pub fn load(files: &[(&str, &str)]) -> Vec<WorkflowRecord> {
    let mut parsed: Vec<WorkflowDefinition> = files
        .iter()
        .filter_map(|(stem, text)| match parse_workflow(stem, text) {
            Ok(definition) => Some(definition),
            Err(err) => {
                tracing::warn!(file = %format!("catalog/workflows/{stem}.md"), error = %err, "skipping a default workflow that does not parse");
                None
            }
        })
        .collect();
    parsed.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    parsed.into_iter().map(record).collect()
}

/// The default workflows, in list order.
#[must_use]
pub fn defaults() -> &'static [WorkflowRecord] {
    DEFAULTS.get_or_init(|| load(WORKFLOW_FILES))
}

/// The default workflow with this id, if any.
#[must_use]
pub fn default_workflow(id: &str) -> Option<&'static WorkflowRecord> {
    defaults().iter().find(|w| w.id == id)
}

#[cfg(test)]
mod tests {
    use yulius_domain::cd::production_instruction;

    use super::*;

    /// Every shipped file parses (a malformed one fails here, not silently at runtime), the build
    /// script embedded exactly the directory's files, and ids are unique.
    #[test]
    fn the_shipped_catalog_is_valid() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../catalog/workflows");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.ends_with(".md") && n != "README.md")
            .map(|n| n.trim_end_matches(".md").to_owned())
            .collect();
        on_disk.sort();
        let embedded: Vec<String> = WORKFLOW_FILES
            .iter()
            .map(|(s, _)| (*s).to_owned())
            .collect();
        assert_eq!(embedded, on_disk, "build.rs embeds every catalog file");
        for (stem, text) in WORKFLOW_FILES {
            if let Err(err) = parse_workflow(stem, text) {
                return Err(format!("catalog/workflows/{stem}.md {err}").into());
            }
        }
        assert_eq!(defaults().len(), WORKFLOW_FILES.len());
        let mut ids: Vec<&str> = defaults().iter().map(|w| w.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), defaults().len());
        assert!(
            defaults()
                .iter()
                .all(|w| w.builtin && w.sandbox_id.is_none())
        );
        // Tasks and automations created before the catalog refer to `wf-feature`.
        assert!(default_workflow("wf-feature").is_some_and(|w| w.require_spec));
        // A confirmed defect is reported as an issue (SPEC-042 DSC-016).
        assert!(default_workflow("wf-fix").is_some_and(|w| {
            w.instructions
                .contains("Once the defect is confirmed, report it")
                && w.instructions.contains("never does")
        }));
        // The session hook takes a workflow's instructions out of the briefing's budget, so a
        // longer workflow leaves less for the briefing (#555). None grows past the largest before
        // SPEC-174 (Infra-bootstrap, 1,444 bytes); the briefing's tests assume 1.7 KB.
        for w in defaults() {
            let block = format!("\n\n## Workflow instructions\n\n{}\n", w.instructions);
            assert!(block.len() <= 1_444, "{} is {} bytes", w.id, block.len());
        }
        Ok(())
    }

    /// Only a person promotes and deploys production (ADR-0110, #556): no shipped workflow, in
    /// its description, steps or instructions, tells an agent to promote or to deploy or verify
    /// production.
    #[test]
    fn no_workflow_tells_agents_to_promote() -> Result<(), Box<dyn std::error::Error>> {
        for (stem, text) in WORKFLOW_FILES {
            if let Some(word) = production_instruction(text) {
                return Err(format!("catalog/workflows/{stem}.md tells agents: {word}").into());
            }
        }
        // Delivery still ends somewhere: dev deployed when asked, production a person's.
        for id in ["wf-feature", "wf-fix"] {
            let w = default_workflow(id).ok_or(id)?;
            assert!(w.instructions.contains("`cd_deploy`"), "{id}");
            assert!(w.instructions.contains("production is a person's"), "{id}");
        }
        Ok(())
    }

    #[test]
    fn order_then_name_and_bad_files_skipped() {
        let file = |name: &str, order: &str| {
            format!(
                "---\nname: {name}\ndescription: d\nwhen_to_use: w\n{order}---\n## Steps\n- s\n## Instructions\n"
            )
        };
        let (b, a, late, bad) = (
            file("Beta", "order: 5\n"),
            file("alpha", "order: 5\n"),
            file("Late", ""),
            "no frontmatter".to_owned(),
        );
        let loaded = load(&[("late", &late), ("b", &b), ("bad", &bad), ("a", &a)]);
        let ids: Vec<&str> = loaded.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["wf-a", "wf-b", "wf-late"]);
        assert_eq!(loaded[0].steps, ["s"]);
        assert_eq!(loaded[0].when_to_use, "w");
    }
}
