//! Every sandbox's installed skills follow the instance's catalog (SPEC-507, ADR-0364): at the
//! server's start, after the catalog is extracted, and when a skill is switched for a sandbox,
//! not only at a run's start there.
//!
//! The run start's install (`prepare_run_extras` in the supervisor) writes every skill; a refresh
//! writes only the copies that differ, so a start with nothing changed writes nothing and says
//! nothing. Both hold the launch lock, so two installs never meet in one home.

use std::path::{Path, PathBuf};

use crate::context::App;
use crate::model::SandboxRecord;
use crate::ports::harness::SkillsRefresh;

/// The audit action of a sandbox whose skills a refresh changed.
pub const REFRESHED_AUDIT: &str = "skills.refreshed";

/// The skills folder in a sandbox's home, relative to the sandbox directory.
pub(crate) fn skills_dir_below(sandbox_dir: &Path, home: &Path) -> PathBuf {
    // The agent owns its home: the skills folder is reached from the sandbox directory without
    // following links (#657).
    home.strip_prefix(sandbox_dir)
        .unwrap_or(Path::new("home"))
        .join(".claude/skills")
}

impl App {
    /// What a server start does with skills (SPEC-507 SKR-001): extracts the embedded packs, then
    /// brings every sandbox's installed copies to the set a run there would get. Called before
    /// background work, queued runs and an update's resume start, so a task the update froze
    /// goes on with the new skills. Failures are logged; the start goes on.
    pub async fn skills_at_start(&self) {
        match self.install_skill_catalog().await {
            Ok(files) => tracing::debug!(files, "skill catalog ready"),
            Err(err) => {
                tracing::error!(error = %err, "cannot install the skill catalog");
                return;
            }
        }
        let sandboxes = match self.deps.store.sandboxes().await {
            Ok(sandboxes) => sandboxes,
            Err(err) => {
                tracing::warn!(error = %err, "sandboxes' skills not refreshed");
                return;
            }
        };
        let mut refreshed = 0_usize;
        for sandbox in &sandboxes {
            if let Some(report) = self.refresh_sandbox_skills(sandbox, "server start").await
                && !report.is_empty()
            {
                refreshed += 1;
            }
        }
        if refreshed > 0 {
            tracing::info!(sandboxes = refreshed, "installed skills refreshed");
        }
    }

    /// Brings `sandbox`'s installed skills to its set, writing only what differs (SPEC-507
    /// SKR-001..005): `None` when it has no home yet (its first run installs them) or the refresh
    /// failed (logged), else what changed. A change is audited as [`REFRESHED_AUDIT`] with `why`.
    pub(crate) async fn refresh_sandbox_skills(
        &self,
        sandbox: &SandboxRecord,
        why: &str,
    ) -> Option<SkillsRefresh> {
        let root = self.sandbox_dir(&sandbox.id);
        let home = self.sandbox_home(&sandbox.id);
        // A home is a folder the first run makes; anything else there is never written through.
        match std::fs::symlink_metadata(&home) {
            Ok(meta) if meta.is_dir() => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
            Ok(_) => {
                tracing::warn!(sandbox = %sandbox.slug, "skill refresh failed: the home is not a folder");
                return None;
            }
            Err(err) => {
                tracing::warn!(error = %err, sandbox = %sandbox.slug, "skill refresh failed");
                return None;
            }
        }
        let skills = match self.skills_for_run(sandbox).await {
            Ok(skills) => skills,
            Err(err) => {
                tracing::warn!(error = %err, sandbox = %sandbox.slug, "cannot list the sandbox's skills");
                return None;
            }
        };
        let skills_dir = skills_dir_below(&root, &home);
        crate::race::reached(crate::race::Point::SkillsRefreshQueued, &root);
        let one_install_at_a_time = self.launch_lock().await;
        let harness = self.deps.harness.clone();
        let report = tokio::task::spawn_blocking(move || {
            let _held = one_install_at_a_time;
            crate::race::reached(crate::race::Point::SkillsRefreshLocked, &root);
            harness.refresh_skills(&skills, &root, &skills_dir)
        })
        .await;
        let report = match report {
            Ok(Ok(report)) => report,
            Ok(Err(err)) => {
                tracing::warn!(error = %err, sandbox = %sandbox.slug, "skill refresh failed");
                return None;
            }
            Err(err) => {
                tracing::warn!(error = %err, sandbox = %sandbox.slug, "skill refresh failed");
                return None;
            }
        };
        if !report.is_empty() {
            let detail = refresh_detail(why, self.build_commit(), &report);
            tracing::info!(sandbox = %sandbox.slug, %detail, "installed skills refreshed");
            if let Err(err) = self
                .deps
                .store
                .audit(
                    self.now(),
                    "system",
                    REFRESHED_AUDIT,
                    Some(&sandbox.slug),
                    Some(&detail),
                )
                .await
            {
                tracing::warn!(error = %err, sandbox = %sandbox.slug, "skill refresh not audited");
            }
        }
        Some(report)
    }
}

/// The audit detail of a refresh: why, the server's commit, what was installed and removed.
fn refresh_detail(why: &str, build: Option<&str>, report: &SkillsRefresh) -> String {
    let mut detail = format!("{why} at {}", build.unwrap_or("an unknown build"));
    if !report.changed.is_empty() {
        detail.push_str("; installed ");
        detail.push_str(&report.changed.join(", "));
    }
    if !report.removed.is_empty() {
        detail.push_str("; removed ");
        detail.push_str(&report.removed.join(", "));
    }
    detail
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_detail_names_why_the_build_and_the_skills() {
        let report = SkillsRefresh {
            changed: vec!["om-fix".into(), "om-code-review".into()],
            removed: vec!["old".into()],
        };
        assert_eq!(
            refresh_detail("server start", Some("ffa950b0c"), &report),
            "server start at ffa950b0c; installed om-fix, om-code-review; removed old"
        );
        let only_removed = SkillsRefresh {
            changed: Vec::new(),
            removed: vec!["old".into()],
        };
        assert_eq!(
            refresh_detail("skill switched", None, &only_removed),
            "skill switched at an unknown build; removed old"
        );
    }

    #[test]
    fn the_skills_folder_is_below_the_home() {
        assert_eq!(
            skills_dir_below(Path::new("/d/s"), Path::new("/d/s/home")),
            PathBuf::from("home/.claude/skills")
        );
        assert_eq!(
            skills_dir_below(Path::new("/d/s"), Path::new("/elsewhere")),
            PathBuf::from("home/.claude/skills")
        );
    }
}
