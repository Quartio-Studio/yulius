//! The skill packs embedded in the binary (SPEC-001 SKL-003, §7.3; SPEC-002 ADR-19).
//!
//! Source: `catalog/skills/<pack>/` at the repository root. Yulius's own skills (`yulius`), the
//! Open Mercato skills adapted to be universal and autonomous (`open-mercato`, MIT) and two skills
//! adapted from obra/superpowers (`superpowers`, MIT). Each pack keeps its license and records
//! its upstream in its README. The file lists are checked against the catalog directories by a
//! unit test, so a file added to or removed from the catalog without updating this module fails
//! the build's tests. Generated layout: keep one entry per file, sorted by path.

use super::SkillSource;

/// One embedded pack.
#[derive(Debug)]
pub struct Pack {
    /// Directory under `catalog/skills/` and `<data_dir>/skills/`.
    pub dir: &'static str,
    /// Source reported for its skills.
    pub source: SkillSource,
    /// Upstream repository and pinned commit the pack was adapted from.
    pub upstream: Option<(&'static str, &'static str)>,
    /// Its skills, in display order.
    pub skills: &'static [&'static str],
    /// Every file as (path relative to the pack directory, contents).
    pub files: &'static [(&'static str, &'static [u8])],
}

/// The packs, in precedence order when two share a skill name (they do not; a test checks).
pub const PACKS: [Pack; 3] = [
    Pack {
        dir: "yulius",
        source: SkillSource::Yulius,
        upstream: None,
        skills: &["delivery-pipeline", "git-hygiene", "infra-bootstrap"],
        files: &[
            (
                "README.md",
                include_bytes!("../../../../catalog/skills/yulius/README.md"),
            ),
            (
                "delivery-pipeline/SKILL.md",
                include_bytes!("../../../../catalog/skills/yulius/delivery-pipeline/SKILL.md"),
            ),
            (
                "git-hygiene/SKILL.md",
                include_bytes!("../../../../catalog/skills/yulius/git-hygiene/SKILL.md"),
            ),
            (
                "infra-bootstrap/SKILL.md",
                include_bytes!("../../../../catalog/skills/yulius/infra-bootstrap/SKILL.md"),
            ),
        ],
    },
    Pack {
        dir: "open-mercato",
        source: SkillSource::OpenMercato,
        upstream: Some((
            "https://github.com/open-mercato/skills",
            "3fc5a1fbd84d90c6f052f3c97cb1e54d2660eb22",
        )),
        skills: &[
            "om-brainstorm",
            "om-spec-writing",
            "om-code-review",
            "om-check-and-commit",
            "om-root-cause",
            "om-fix",
            "om-ux-shape",
        ],
        files: &[
            (
                "LICENSE",
                include_bytes!("../../../../catalog/skills/open-mercato/LICENSE"),
            ),
            (
                "README.md",
                include_bytes!("../../../../catalog/skills/open-mercato/README.md"),
            ),
            (
                "om-brainstorm/SKILL.md",
                include_bytes!("../../../../catalog/skills/open-mercato/om-brainstorm/SKILL.md"),
            ),
            (
                "om-brainstorm/references/brief-template.md",
                include_bytes!(
                    "../../../../catalog/skills/open-mercato/om-brainstorm/references/brief-template.md"
                ),
            ),
            (
                "om-brainstorm/references/challenger-prompt.md",
                include_bytes!(
                    "../../../../catalog/skills/open-mercato/om-brainstorm/references/challenger-prompt.md"
                ),
            ),
            (
                "om-check-and-commit/SKILL.md",
                include_bytes!(
                    "../../../../catalog/skills/open-mercato/om-check-and-commit/SKILL.md"
                ),
            ),
            (
                "om-code-review/SKILL.md",
                include_bytes!("../../../../catalog/skills/open-mercato/om-code-review/SKILL.md"),
            ),
            (
                "om-code-review/references/output-format.md",
                include_bytes!(
                    "../../../../catalog/skills/open-mercato/om-code-review/references/output-format.md"
                ),
            ),
            (
                "om-code-review/references/review-checklist.md",
                include_bytes!(
                    "../../../../catalog/skills/open-mercato/om-code-review/references/review-checklist.md"
                ),
            ),
            (
                "om-fix/SKILL.md",
                include_bytes!("../../../../catalog/skills/open-mercato/om-fix/SKILL.md"),
            ),
            (
                "om-root-cause/SKILL.md",
                include_bytes!("../../../../catalog/skills/open-mercato/om-root-cause/SKILL.md"),
            ),
            (
                "om-spec-writing/SKILL.md",
                include_bytes!("../../../../catalog/skills/open-mercato/om-spec-writing/SKILL.md"),
            ),
            (
                "om-spec-writing/references/data-model-graph.md",
                include_bytes!(
                    "../../../../catalog/skills/open-mercato/om-spec-writing/references/data-model-graph.md"
                ),
            ),
            (
                "om-ux-shape/SKILL.md",
                include_bytes!("../../../../catalog/skills/open-mercato/om-ux-shape/SKILL.md"),
            ),
            (
                "om-ux-shape/references/ai-interaction.md",
                include_bytes!(
                    "../../../../catalog/skills/open-mercato/om-ux-shape/references/ai-interaction.md"
                ),
            ),
            (
                "om-ux-shape/references/decision-framework.md",
                include_bytes!(
                    "../../../../catalog/skills/open-mercato/om-ux-shape/references/decision-framework.md"
                ),
            ),
            (
                "om-ux-shape/references/evidence-tiers.md",
                include_bytes!(
                    "../../../../catalog/skills/open-mercato/om-ux-shape/references/evidence-tiers.md"
                ),
            ),
            (
                "om-ux-shape/references/hai-guidelines.md",
                include_bytes!(
                    "../../../../catalog/skills/open-mercato/om-ux-shape/references/hai-guidelines.md"
                ),
            ),
            (
                "om-ux-shape/references/human-value-metrics.md",
                include_bytes!(
                    "../../../../catalog/skills/open-mercato/om-ux-shape/references/human-value-metrics.md"
                ),
            ),
            (
                "om-ux-shape/references/quality-rubric.md",
                include_bytes!(
                    "../../../../catalog/skills/open-mercato/om-ux-shape/references/quality-rubric.md"
                ),
            ),
            (
                "om-ux-shape/references/report-templates.md",
                include_bytes!(
                    "../../../../catalog/skills/open-mercato/om-ux-shape/references/report-templates.md"
                ),
            ),
            (
                "om-ux-shape/references/reward-and-mental-models.md",
                include_bytes!(
                    "../../../../catalog/skills/open-mercato/om-ux-shape/references/reward-and-mental-models.md"
                ),
            ),
        ],
    },
    Pack {
        dir: "superpowers",
        source: SkillSource::Superpowers,
        upstream: Some((
            "https://github.com/obra/superpowers",
            "bb92a77741419a4ab5f06e711a283343f1ada0c3",
        )),
        skills: &["test-driven-development", "verification-before-completion"],
        files: &[
            (
                "LICENSE",
                include_bytes!("../../../../catalog/skills/superpowers/LICENSE"),
            ),
            (
                "README.md",
                include_bytes!("../../../../catalog/skills/superpowers/README.md"),
            ),
            (
                "test-driven-development/SKILL.md",
                include_bytes!(
                    "../../../../catalog/skills/superpowers/test-driven-development/SKILL.md"
                ),
            ),
            (
                "test-driven-development/condition-based-waiting.md",
                include_bytes!(
                    "../../../../catalog/skills/superpowers/test-driven-development/condition-based-waiting.md"
                ),
            ),
            (
                "test-driven-development/writing-good-tests.md",
                include_bytes!(
                    "../../../../catalog/skills/superpowers/test-driven-development/writing-good-tests.md"
                ),
            ),
            (
                "verification-before-completion/SKILL.md",
                include_bytes!(
                    "../../../../catalog/skills/superpowers/verification-before-completion/SKILL.md"
                ),
            ),
        ],
    },
];
