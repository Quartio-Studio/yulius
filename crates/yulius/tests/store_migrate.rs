//! `yulius store-migrate --db <file>` (SPEC-591 STG-004, ADR-0430): the release's own migrations
//! applied to a copy of a store an older release wrote, so staging's deploy measures the
//! migration before it stops anything.

use std::path::Path;
use std::process::Command;

use crate::support::TestResult;

fn yulius(db: &Path) -> std::io::Result<std::process::Output> {
    Command::new(env!("CARGO_BIN_EXE_yulius"))
        .args(["store-migrate", "--db"])
        .arg(db)
        .output()
}

fn schema(db: &Path) -> rusqlite::Result<i64> {
    rusqlite::Connection::open(db)?.query_row("PRAGMA user_version", [], |row| row.get(0))
}

#[test]
fn an_older_store_is_migrated_to_this_release_and_keeps_its_rows() -> TestResult {
    let dir = tempfile::tempdir()?;
    let db = dir.path().join("yulius.db");
    yulius_store::create_at_version(&db, 70)?;
    rusqlite::Connection::open(&db)?.execute(
        "INSERT INTO users (id, display_name, created_at) VALUES ('u1', 'Octo', 1)",
        [],
    )?;
    let out = yulius(&db)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let latest = yulius_store::schema_version();
    assert_eq!(stdout.trim(), format!("schema {latest} (from 70)"));
    assert_eq!(schema(&db)?, i64::from(latest));
    let users: i64 =
        rusqlite::Connection::open(&db)?
            .query_row("SELECT count(*) FROM users", [], |r| r.get(0))?;
    assert_eq!(users, 1);
    Ok(())
}

#[test]
fn a_missing_or_newer_store_is_refused_and_left_as_it_was() -> TestResult {
    let dir = tempfile::tempdir()?;
    let missing = dir.path().join("none.db");
    let out = yulius(&missing)?;
    assert!(!out.status.success());
    assert!(!missing.exists(), "no store is created");

    let newer = dir.path().join("newer.db");
    yulius_store::create_at_version(&newer, 3)?;
    let ahead = i64::from(yulius_store::schema_version()) + 1;
    rusqlite::Connection::open(&newer)?.pragma_update(None, "user_version", ahead)?;
    let out = yulius(&newer)?;
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("newer"),
        "{out:?}"
    );
    assert_eq!(schema(&newer)?, ahead);
    Ok(())
}
