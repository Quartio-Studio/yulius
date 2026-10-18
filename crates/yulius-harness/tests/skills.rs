//! Skill installation contracts: the Claude home, atomic replacement, pruning and bounds
//! (SPEC-001 SKL-001/004, AC-SKL-1).

use std::fs;
use std::path::{Path, PathBuf};

use yulius_harness::skills::{
    MANAGED_MARKER, MAX_SKILL_BYTES, SkillDir, SkillsError, install_skills,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn skill(root: &Path, name: &str, body: &str) -> std::io::Result<SkillDir> {
    let path = root.join("catalog").join(name);
    fs::create_dir_all(path.join("references"))?;
    fs::write(
        path.join("SKILL.md"),
        format!("---\nname: {name}\n---\n{body}\n"),
    )?;
    fs::write(path.join("references/guide.md"), "guide")?;
    Ok(SkillDir {
        name: name.into(),
        path,
    })
}

/// The skills folder below the sandbox directory.
const SKILLS: &str = "home/.claude/skills";

fn home(root: &Path) -> PathBuf {
    root.join(SKILLS)
}

#[test]
fn installs_into_the_claude_home_and_replaces_stale_copies() -> TestResult {
    let dir = tempfile::tempdir()?;
    let claude = home(dir.path());
    let review = skill(dir.path(), "code-review", "v1")?;
    let report = install_skills(std::slice::from_ref(&review), dir.path(), Path::new(SKILLS))?;
    assert_eq!(report.installed, ["code-review"]);
    {
        let home = &claude;
        let installed = home.join("code-review");
        assert!(fs::read_to_string(installed.join("SKILL.md"))?.contains("v1"));
        assert_eq!(
            fs::read_to_string(installed.join("references/guide.md"))?,
            "guide"
        );
        assert!(installed.join(MANAGED_MARKER).is_file());
    }

    // A stale copy (with a file the new version no longer has) is replaced as a whole.
    fs::write(claude.join("code-review/stale.md"), "old")?;
    fs::write(
        review.path.join("SKILL.md"),
        "---\nname: code-review\n---\nv2\n",
    )?;
    install_skills(std::slice::from_ref(&review), dir.path(), Path::new(SKILLS))?;
    {
        let home = &claude;
        assert!(fs::read_to_string(home.join("code-review/SKILL.md"))?.contains("v2"));
        assert!(!home.join("code-review/stale.md").exists());
    }
    // No staging or backup directories are left behind.
    {
        let home = &claude;
        let names: Vec<String> = fs::read_dir(home)?
            .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect::<Result<_, _>>()?;
        assert_eq!(names, ["code-review"]);
    }
    Ok(())
}

#[test]
fn prunes_only_managed_skills_that_left_the_catalog() -> TestResult {
    let dir = tempfile::tempdir()?;
    let claude = home(dir.path());
    let a = skill(dir.path(), "a-skill", "a")?;
    let b = skill(dir.path(), "b-skill", "b")?;
    install_skills(&[a.clone(), b], dir.path(), Path::new(SKILLS))?;
    // A skill the user installed by hand has no marker and must survive.
    fs::create_dir_all(claude.join("personal"))?;
    fs::write(claude.join("personal/SKILL.md"), "mine")?;
    let report = install_skills(&[a], dir.path(), Path::new(SKILLS))?;
    assert_eq!(report.removed, ["b-skill"]);
    assert!(!claude.join("b-skill").exists());
    assert!(claude.join("personal/SKILL.md").is_file());
    assert!(claude.join("a-skill/SKILL.md").is_file());
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlinks_are_copied_only_inside_the_skill() -> TestResult {
    let dir = tempfile::tempdir()?;
    let claude = home(dir.path());
    let linked = skill(dir.path(), "linked", "x")?;
    fs::write(dir.path().join("secret.txt"), "outside")?;
    std::os::unix::fs::symlink(
        dir.path().join("secret.txt"),
        linked.path.join("escape.txt"),
    )?;
    std::os::unix::fs::symlink(
        linked.path.join("references/guide.md"),
        linked.path.join("alias.md"),
    )?;
    std::os::unix::fs::symlink(&linked.path, linked.path.join("loop"))?;
    let report = install_skills(std::slice::from_ref(&linked), dir.path(), Path::new(SKILLS))?;
    assert_eq!(
        report.skipped_links, 2,
        "escaping file link and directory link"
    );
    let installed = claude.join("linked");
    assert!(!installed.join("escape.txt").exists());
    assert!(!installed.join("loop").exists());
    assert_eq!(fs::read_to_string(installed.join("alias.md"))?, "guide");
    assert!(
        !fs::symlink_metadata(installed.join("alias.md"))?
            .file_type()
            .is_symlink()
    );
    Ok(())
}

#[test]
fn invalid_sources_fail_before_anything_is_written() -> TestResult {
    let dir = tempfile::tempdir()?;
    let claude = home(dir.path());
    let good = skill(dir.path(), "good", "g")?;
    let no_manifest = SkillDir {
        name: "empty".into(),
        path: dir.path().join("catalog/empty"),
    };
    fs::create_dir_all(&no_manifest.path)?;
    assert!(matches!(
        install_skills(&[good.clone(), no_manifest], dir.path(), Path::new(SKILLS)),
        Err(SkillsError::MissingSkillMd(name)) if name == "empty"
    ));
    assert!(!claude.exists(), "validation happens before any write");
    for bad in ["../escape", ".hidden", "a/b", ""] {
        let invalid = SkillDir {
            name: bad.into(),
            path: good.path.clone(),
        };
        assert!(matches!(
            install_skills(&[invalid], dir.path(), Path::new(SKILLS)),
            Err(SkillsError::InvalidName(_))
        ));
    }
    assert!(matches!(
        install_skills(&[good.clone(), good], dir.path(), Path::new(SKILLS)),
        Err(SkillsError::InvalidName(name)) if name == "good"
    ));
    Ok(())
}

#[test]
fn oversized_skill_keeps_the_previous_copy() -> TestResult {
    let dir = tempfile::tempdir()?;
    let claude = home(dir.path());
    let big = skill(dir.path(), "big", "v1")?;
    install_skills(std::slice::from_ref(&big), dir.path(), Path::new(SKILLS))?;
    let file = fs::File::create(big.path.join("blob.bin"))?;
    file.set_len(MAX_SKILL_BYTES + 1)?;
    assert!(matches!(
        install_skills(std::slice::from_ref(&big), dir.path(), Path::new(SKILLS)),
        Err(SkillsError::TooLarge(name)) if name == "big"
    ));
    assert!(fs::read_to_string(claude.join("big/SKILL.md"))?.contains("v1"));
    assert!(!claude.join("big/blob.bin").exists());
    let leftovers = fs::read_dir(&claude)?.count();
    assert_eq!(leftovers, 1, "the failed staging copy is removed");
    Ok(())
}

#[test]
fn a_linked_skills_folder_is_never_written_through() -> TestResult {
    let dir = tempfile::tempdir()?;
    let review = skill(dir.path(), "code-review", "v1")?;
    // The agent owns its home: it replaced its skills folder with a link to a host folder that
    // holds a managed-looking copy and a staging-named folder (#657).
    let outside = dir.path().join("host");
    fs::create_dir_all(outside.join("stale/references"))?;
    fs::write(outside.join("stale").join(MANAGED_MARKER), "x")?;
    fs::create_dir_all(outside.join(".x.yulius-stage-1"))?;
    fs::create_dir_all(dir.path().join("home/.claude"))?;
    std::os::unix::fs::symlink(&outside, home(dir.path()))?;
    let result = install_skills(std::slice::from_ref(&review), dir.path(), Path::new(SKILLS));
    assert!(result.is_err(), "{result:?}");
    let mut names: Vec<String> = fs::read_dir(&outside)?
        .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    names.sort();
    assert_eq!(
        names,
        [".x.yulius-stage-1", "stale"],
        "the host folder is untouched"
    );
    assert!(outside.join("stale").join(MANAGED_MARKER).is_file());
    // A link higher up (the whole `.claude`) is refused the same way.
    fs::remove_file(home(dir.path()))?;
    fs::remove_dir(dir.path().join("home/.claude"))?;
    std::os::unix::fs::symlink(&outside, dir.path().join("home/.claude"))?;
    assert!(install_skills(&[review], dir.path(), Path::new(SKILLS)).is_err());
    assert!(!outside.join("skills").exists());
    Ok(())
}
