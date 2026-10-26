use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn the_manifest_has_a_fixed_length_whatever_its_total() -> TestResult {
    let mut m = Manifest {
        format: FORMAT,
        data_dir: "/var/lib/yulius".into(),
        ..Manifest::default()
    };
    let small = m.to_bytes()?;
    m.total_bytes = u64::MAX;
    m.entries = u64::MAX;
    let big = m.to_bytes()?;
    assert_eq!(small.len() as u64, MANIFEST_LEN);
    assert_eq!(big.len() as u64, MANIFEST_LEN);
    let back: Manifest = serde_json::from_slice(&big)?;
    assert_eq!(back, m);
    Ok(())
}

#[test]
fn a_manifest_that_does_not_fit_is_an_error() -> TestResult {
    let m = Manifest {
        data_dir: "x".repeat(9000),
        ..Manifest::default()
    };
    assert!(m.to_bytes().is_err());
    Ok(())
}

#[test]
fn users_are_found_by_id_and_name() -> TestResult {
    let passwd =
        "root:x:0:0:root:/root:/bin/bash\nyulius:x:998:997::/var/lib/yulius:/usr/sbin/nologin\n";
    assert_eq!(user_by_uid(passwd, 998), Some(("yulius".into(), 997)));
    assert_eq!(user_by_uid(passwd, 5), None);
    assert_eq!(user_by_name(passwd, "yulius"), Some((998, 997)));
    assert_eq!(user_by_name(passwd, "yuliu"), None);
    Ok(())
}

#[test]
fn os_release_is_id_and_version() -> TestResult {
    let text = "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\nID_LIKE=debian\n";
    assert_eq!(os_release(text).as_deref(), Some("ubuntu 24.04"));
    assert_eq!(os_release("NAME=x\n"), None);
    Ok(())
}

#[test]
fn the_progress_line_says_done_total_rate_and_time_left() -> TestResult {
    let p = Progress::new("Exported", 10 * 1024 * 1024 * 1024);
    let line = p.line(1024 * 1024 * 1024, Duration::from_secs(10));
    assert_eq!(
        line,
        "Exported 1.0 GiB of 10.0 GiB (10%), 102.4 MiB/s, 0h01m30s left"
    );
    Ok(())
}
