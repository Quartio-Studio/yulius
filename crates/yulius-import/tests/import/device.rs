//! Device login polling states (docs/api.md CLI device approval).

use std::time::Duration;

use yulius_contracts::import::DeviceStatus;
use yulius_import::{DeviceError, DeviceLoginOptions, RetryPolicy, device_login};

use crate::support::TestResult;
use crate::support::server::{FakeServer, TOKEN, USER_CODE};

fn options(max_wait_s: u64) -> DeviceLoginOptions {
    DeviceLoginOptions {
        poll_interval: Duration::ZERO,
        max_wait: Duration::from_secs(max_wait_s),
        retry: RetryPolicy::immediate(1),
    }
}

#[test]
fn polls_until_approved_and_returns_the_token() -> TestResult {
    let server = FakeServer::start()?;
    server.with(|state| {
        state.device_script = [
            DeviceStatus::Pending,
            DeviceStatus::Pending,
            DeviceStatus::Approved,
        ]
        .into();
    })?;
    let mut shown = Vec::new();
    let token = device_login(
        &server.url,
        &mut |start| shown.push(start.user_code.clone()),
        &options(600),
    )?;
    assert_eq!(token.expose(), TOKEN);
    assert!(!format!("{token:?}").contains(TOKEN));
    assert_eq!(shown, [USER_CODE]);
    assert_eq!(server.with(|state| state.device_polls)?, 3);
    Ok(())
}

#[test]
fn denied_and_expired_requests_end_the_login() -> TestResult {
    let server = FakeServer::start()?;
    server
        .with(|state| state.device_script = [DeviceStatus::Pending, DeviceStatus::Denied].into())?;
    let denied = device_login(&server.url, &mut |_| {}, &options(600));
    assert!(matches!(denied, Err(DeviceError::Denied)));

    server.with(|state| state.device_script = [DeviceStatus::Expired].into())?;
    let expired = device_login(&server.url, &mut |_| {}, &options(600));
    assert!(matches!(expired, Err(DeviceError::Expired)));
    Ok(())
}

#[test]
fn waiting_is_bounded() -> TestResult {
    let server = FakeServer::start()?;
    // The script is empty, so every poll answers `pending`.
    let result = device_login(&server.url, &mut |_| {}, &options(3));
    assert!(matches!(result, Err(DeviceError::TimedOut(wait)) if wait == Duration::from_secs(3)));
    assert_eq!(server.with(|state| state.device_polls)?, 3);
    Ok(())
}

#[test]
fn insecure_servers_are_refused_before_any_request() -> TestResult {
    let url = yulius_import::Url::parse("http://yulius.example")?;
    let result = device_login(&url, &mut |_| {}, &options(1));
    assert!(matches!(
        result,
        Err(DeviceError::Api(
            yulius_import::ApiError::InsecureServerUrl(_)
        ))
    ));
    Ok(())
}
