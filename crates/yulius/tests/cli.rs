//! The `yulius` binary's operator commands (SPEC-001 AC-OPS-3; SPEC-002 ADR-01).

use std::process::Command;

use yulius_app::model::UserRecord;
use yulius_domain::ids::UserId;
use yulius_domain::time::Timestamp;
use yulius_store::SqliteStore;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const EXE: &str = env!("CARGO_BIN_EXE_yulius");

#[tokio::test]
async fn backup_writes_a_consistent_copy_and_never_overwrites() -> TestResult {
    let dir = tempfile::tempdir()?;
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data)?;
    {
        let store = SqliteStore::open(&data.join("yulius.db"))?;
        use yulius_app::ports::store::UserStore;
        store
            .insert_user(&UserRecord {
                id: UserId::parse("u-backup")?,
                github_login: Some("backup".into()),
                github_id: None,
                display_name: "Backup".into(),
                avatar_url: None,
                is_superadmin: false,
                is_instance_admin: false,
                password_hash: None,
                created_at: Timestamp(1),
                disabled_at: None,
            })
            .await?;
    }
    let dest = dir.path().join("copy.db");
    let out = Command::new(EXE)
        .args(["backup", "--data-dir"])
        .arg(&data)
        .arg(&dest)
        .output()?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let copy = SqliteStore::open(&dest)?;
    use yulius_app::ports::store::UserStore;
    let user = copy.user_by_login("backup").await?;
    assert_eq!(
        user.map(|u| u.id.as_str().to_owned()),
        Some("u-backup".into())
    );

    let again = Command::new(EXE)
        .args(["backup", "--data"])
        .arg(&data)
        .arg(&dest)
        .output()?;
    assert!(
        !again.status.success(),
        "an existing destination is refused"
    );
    Ok(())
}

#[test]
fn doctor_fails_on_a_data_dir_too_long_for_session_sockets() -> TestResult {
    let long = std::path::Path::new("/tmp").join("y".repeat(90));
    let out = Command::new(EXE)
        .args(["doctor", "--data"])
        .arg(&long)
        .output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success());
    assert!(text.contains("FAIL  session socket paths"), "{text}");
    assert!(text.contains("git"), "{text}");
    Ok(())
}

/// SPEC-205 AC-7: a malformed file is refused before anything is asked or sent, naming the line
/// and never the value.
#[test]
fn secrets_import_refuses_a_malformed_file_without_showing_values() -> TestResult {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("secrets.env");
    std::fs::write(&file, "GOOD=first-value-123\nnot-a-line-secret-456\n")?;
    let out = Command::new(EXE)
        .args([
            "secrets",
            "import",
            "--to",
            "team",
            "--server",
            "http://127.0.0.1:9",
        ])
        .arg(&file)
        .env("HOME", dir.path())
        .stdin(std::process::Stdio::null())
        .output()?;
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(1), "{all}");
    assert!(all.contains("line 2"), "{all}");
    assert!(
        !all.contains("first-value-123") && !all.contains("secret-456"),
        "{all}"
    );
    assert!(!all.contains("Type yes"), "{all}");
    Ok(())
}
