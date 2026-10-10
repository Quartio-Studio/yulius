//! `GitHubClient` behavior against the in-process `FakeGitHub` (no real GitHub calls).

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use yulius_domain::time::Timestamp;
use yulius_github::fake::{FakeCheckRun, FakeFault, FakeGitHub, FakeStatus};
use yulius_github::{
    AccessLevel, CheckSource, ChecksSummary, Clock, CommitStatus, CompareStatus, CreateRepo,
    GitHubClient,
    GitHubConfig, GitHubError, GitHubPermissions, MergeMethod, NewPull, OwnerKind, PullFilter,
    PullState, RepoAccess, RetryPolicy, RulesetSpec, Secret, SystemClock, Url,
};

type TestResult = Result<(), Box<dyn Error>>;

/// A settable clock for token-expiry tests.
struct TestClock(AtomicI64);

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        Timestamp(self.0.load(Ordering::SeqCst))
    }
}

async fn setup() -> Result<(FakeGitHub, GitHubClient), Box<dyn Error>> {
    let fake = FakeGitHub::start().await?;
    let client = GitHubClient::new(fake.config())?;
    Ok((fake, client))
}

fn perms(admin: bool, maintain: bool, push: bool, triage: bool, pull: bool) -> GitHubPermissions {
    GitHubPermissions {
        admin,
        maintain,
        push,
        triage,
        pull,
    }
}

fn writer() -> GitHubPermissions {
    perms(false, false, true, true, true)
}

#[tokio::test]
async fn oauth_exchange_refresh_and_current_user() -> TestResult {
    let (fake, client) = setup().await?;
    let id = fake.add_user("ada", Some("Ada Lovelace"));
    fake.add_oauth_code("code-123", "ada");
    let redirect = "http://127.0.0.1/api/auth/github/callback";

    let before = SystemClock.now();
    let tokens = client.exchange_code("code-123", redirect).await?;
    assert!(tokens.access.expose().starts_with("ghu_"));
    let refresh = tokens.refresh.clone().ok_or("missing refresh token")?;
    let expires = tokens.expires_at.ok_or("missing expiry")?;
    assert!(expires.millis_since(before) >= 8 * 3600 * 1000 - 60_000);
    assert!(tokens.refresh_expires_at.is_some());

    let user = client.current_user(&tokens.access).await?;
    assert_eq!((user.id, user.login.as_str()), (id, "ada"));
    assert_eq!(user.name.as_deref(), Some("Ada Lovelace"));

    let renewed = client.refresh_user_token(&refresh).await?;
    assert_ne!(renewed.access, tokens.access);
    assert_eq!(client.current_user(&renewed.access).await?.login, "ada");
    // Refresh tokens are single use; codes too.
    assert_eq!(
        client.refresh_user_token(&refresh).await,
        Err(GitHubError::OAuth("bad_refresh_token".into()))
    );
    assert_eq!(
        client.exchange_code("code-123", redirect).await,
        Err(GitHubError::OAuth("bad_verification_code".into()))
    );
    assert_eq!(
        client.current_user(&Secret::new("ghu_notvalid")).await,
        Err(GitHubError::Unauthorized)
    );

    let token_request = fake
        .requests()
        .into_iter()
        .find(|r| r.web && r.path == "/login/oauth/access_token")
        .ok_or("no token request")?;
    assert_eq!(token_request.header("accept"), Some("application/json"));
    assert!(token_request.body.contains("client_id=Iv1.fakeyuliusapp"));
    assert!(token_request.body.contains("code=code-123"));
    fake.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn authorize_url_carries_client_state_and_redirect() -> TestResult {
    let (fake, client) = setup().await?;
    let url = Url::parse(&client.authorize_url(
        "st4te",
        "https://yulius.example/api/auth/github/callback",
        false,
    )?)?;
    assert_eq!(url.path(), "/web/login/oauth/authorize");
    let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    assert!(pairs.contains(&("client_id".into(), "Iv1.fakeyuliusapp".into())));
    assert!(pairs.contains(&("state".into(), "st4te".into())));
    assert!(pairs.contains(&(
        "redirect_uri".into(),
        "https://yulius.example/api/auth/github/callback".into()
    )));
    assert!(
        !pairs.iter().any(|(k, _)| k == "prompt"),
        "no picker by default"
    );
    let picker = Url::parse(&client.authorize_url("st4te", "https://x/cb", true)?)?;
    let pairs: Vec<(String, String)> = picker.query_pairs().into_owned().collect();
    assert!(pairs.contains(&("prompt".into(), "select_account".into())));
    let mut unconfigured = fake.config();
    unconfigured.app = None;
    let bare = GitHubClient::new(unconfigured)?;
    assert_eq!(
        bare.authorize_url("s", "https://x/cb", false),
        Err(GitHubError::NotConfigured)
    );
    assert!(matches!(bare.app_jwt(), Err(GitHubError::NotConfigured)));
    Ok(())
}

#[tokio::test]
async fn permission_mapping_none_read_write_admin() -> TestResult {
    let (fake, client) = setup().await?;
    fake.add_repo("acme", "app", true);
    let cases = [
        ("nobody", None, AccessLevel::None),
        (
            "reader",
            Some(perms(false, false, false, false, true)),
            AccessLevel::Read,
        ),
        (
            "triager",
            Some(perms(false, false, false, true, true)),
            AccessLevel::Read,
        ),
        ("writer", Some(writer()), AccessLevel::Write),
        (
            "maintainer",
            Some(perms(false, true, true, true, true)),
            AccessLevel::Write,
        ),
        (
            "admin",
            Some(perms(true, true, true, true, true)),
            AccessLevel::Admin,
        ),
    ];
    for (login, permission, expected) in cases {
        fake.add_user(login, None);
        if let Some(permission) = permission {
            fake.set_permission("acme", "app", login, permission);
        }
        let token = fake.issue_user_token(login);
        let access = client.repo_permissions(&token, "acme", "app").await?;
        assert_eq!(access.level(), expected, "{login}");
        match (&access, permission) {
            (RepoAccess::NoAccess, None) => {}
            (RepoAccess::Granted { permissions, repo }, Some(p)) => {
                assert_eq!(*permissions, p);
                assert_eq!(repo.full_name, "acme/app");
                assert!(repo.private);
            }
            other => return Err(format!("unexpected mapping for {login}: {other:?}").into()),
        }
    }
    let token = fake.issue_user_token("admin");
    assert_eq!(
        client.repo_permissions(&token, "acme", "missing").await?,
        RepoAccess::NoAccess
    );
    assert_eq!(
        client
            .repo_permissions(&Secret::new("ghu_bogus"), "acme", "app")
            .await,
        Err(GitHubError::Unauthorized)
    );
    assert!(matches!(
        client.repo_permissions(&token, "acme", "../x").await,
        Err(GitHubError::InvalidInput { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn installation_tokens_are_cached_until_near_expiry() -> TestResult {
    let fake = FakeGitHub::start().await?;
    let installation = fake.add_installation("acme", OwnerKind::Organization);
    let clock = Arc::new(TestClock(AtomicI64::new(SystemClock.now().0)));
    let client = GitHubClient::with_clock(fake.config(), clock.clone())?;
    let path = format!("/app/installations/{installation}/access_tokens");

    let first = client.create_installation_token(installation).await?;
    let second = client.create_installation_token(installation).await?;
    assert_eq!(first, second);
    assert_eq!(fake.count("POST", &path), 1);
    // 56 minutes later the cached token is inside the 5-minute margin and is replaced.
    clock.0.fetch_add(56 * 60 * 1000, Ordering::SeqCst);
    fake.advance_time(Duration::from_secs(56 * 60));
    let third = client.create_installation_token(installation).await?;
    assert_ne!(third.token, first.token);
    assert_eq!(fake.count("POST", &path), 2);
    assert!(matches!(
        client.create_installation_token(999_999).await,
        Err(GitHubError::NotFound)
    ));
    Ok(())
}

#[tokio::test]
async fn app_jwt_is_verified_and_installations_are_found() -> TestResult {
    let (fake, client) = setup().await?;
    let org = fake.add_installation("Acme", OwnerKind::Organization);
    let user = fake.add_installation("ada", OwnerKind::User);
    assert_eq!(client.find_installation("acme").await?, Some(org));
    assert_eq!(client.find_installation("ADA").await?, Some(user));
    assert_eq!(client.find_installation("nobody").await?, None);
    fake.add_repo("acme", "app", true);
    fake.add_repo("acme", "site", false);
    fake.add_repo("ada", "notes", true);
    let mut names: Vec<String> = client
        .installation_repositories(org)
        .await?
        .into_iter()
        .map(|r| r.full_name)
        .collect();
    names.sort();
    assert_eq!(names, ["acme/app", "acme/site"]);

    // A JWT signed with a key GitHub does not know is rejected by the fake's verifier.
    let mut wrong = fake.config();
    if let Some(app) = wrong.app.as_mut() {
        app.client_id = "Iv1.someoneelse".into();
    }
    let impostor = GitHubClient::new(wrong)?;
    assert_eq!(
        impostor.find_installation("acme").await,
        Err(GitHubError::Unauthorized)
    );
    Ok(())
}

#[tokio::test]
async fn manifest_flow_builds_manifest_and_converts_code() -> TestResult {
    let (fake, client) = setup().await?;
    let public = Url::parse("https://yulius.example.com/")?;
    let manifest = client.app_manifest(&public, "acme", OwnerKind::Organization)?;
    let permissions = &manifest["default_permissions"];
    for (name, level) in [
        ("contents", "write"),
        ("pull_requests", "write"),
        ("checks", "read"),
        ("statuses", "write"),
        ("workflows", "write"),
        ("administration", "write"),
        ("metadata", "read"),
        ("members", "read"),
    ] {
        assert_eq!(permissions[name], level, "{name}");
    }
    assert_eq!(
        manifest["callback_urls"][0],
        "https://yulius.example.com/api/auth/github/callback"
    );
    assert_eq!(
        manifest["redirect_url"],
        "https://yulius.example.com/api/admin/github/manifest/callback"
    );
    assert_eq!(manifest["public"], false);
    assert!(
        client
            .app_manifest(
                &Url::parse("http://yulius.example.com/")?,
                "acme",
                OwnerKind::User
            )
            .is_err()
    );

    let base = fake.web_base();
    assert_eq!(
        client.manifest_post_url("acme", OwnerKind::Organization)?,
        format!("{base}organizations/acme/settings/apps/new")
    );
    assert_eq!(
        client.manifest_post_url("ada", OwnerKind::User)?,
        format!("{base}settings/apps/new")
    );
    assert!(
        client
            .manifest_post_url_with_state("ada", OwnerKind::User, "csrf1")?
            .ends_with("settings/apps/new?state=csrf1")
    );
    assert!(
        client
            .manifest_post_url("../evil", OwnerKind::User)
            .is_err()
    );

    fake.add_manifest_code("abc123");
    let credentials = client.convert_manifest("abc123").await?;
    assert_eq!(credentials, fake.app_credentials());
    assert_eq!(
        client.convert_manifest("abc123").await,
        Err(GitHubError::NotFound)
    );
    assert_eq!(
        client.install_url()?,
        format!("{base}apps/yulius-fake/installations/new")
    );
    // The converted credentials produce a working App client.
    let converted = GitHubClient::new(GitHubConfig {
        app: Some(credentials),
        ..fake.config()
    })?;
    converted.app_jwt()?;
    Ok(())
}

#[tokio::test]
async fn repository_creation_for_organizations_and_users() -> TestResult {
    let (fake, client) = setup().await?;
    let installation = fake.add_installation("acme", OwnerKind::Organization);
    let token = client.create_installation_token(installation).await?;
    let repo = client
        .create_org_repo(&token.token, "acme", &CreateRepo::private("new-app"))
        .await?;
    assert_eq!(
        (repo.owner.as_str(), repo.name.as_str()),
        ("acme", "new-app")
    );
    assert!(repo.private);
    assert_eq!(repo.default_branch, "main");
    let request = fake
        .requests()
        .into_iter()
        .find(|r| r.path == "/orgs/acme/repos")
        .ok_or("no org repo request")?;
    let body: serde_json::Value = serde_json::from_str(&request.body)?;
    assert_eq!(
        (body["private"].clone(), body["auto_init"].clone()),
        (true.into(), false.into())
    );
    assert!(matches!(
        client
            .create_org_repo(&token.token, "acme", &CreateRepo::private("new-app"))
            .await,
        Err(GitHubError::Invalid { .. })
    ));
    assert!(matches!(
        client
            .create_org_repo(&token.token, "other-org", &CreateRepo::private("x"))
            .await,
        Err(GitHubError::Forbidden { .. })
    ));

    fake.add_user("ada", None);
    let user_token = fake.issue_user_token("ada");
    let personal = client
        .create_user_repo(&user_token, &CreateRepo::private("notes"))
        .await?;
    assert_eq!(personal.full_name, "ada/notes");
    assert!(fake.has_repo("ada", "notes"));
    assert_eq!(
        client
            .repo_permissions(&user_token, "ada", "notes")
            .await?
            .level(),
        AccessLevel::Admin
    );
    let fetched = client.get_repo(&user_token, "ada", "notes").await?;
    assert_eq!(fetched.id, personal.id);
    assert!(matches!(
        client
            .create_user_repo(&user_token, &CreateRepo::private("bad/name"))
            .await,
        Err(GitHubError::InvalidInput { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn rulesets_protect_branches() -> TestResult {
    let (fake, client) = setup().await?;
    fake.add_repo("acme", "app", true);
    fake.add_user("boss", None);
    fake.set_permission("acme", "app", "boss", perms(true, true, true, true, true));
    let token = fake.issue_user_token("boss");
    let spec = RulesetSpec {
        name: "yulius: protect dev".into(),
        branch: "dev".into(),
        require_pr: true,
        allowed_merge_methods: vec![MergeMethod::Squash],
        block_force_push: true,
        block_deletion: true,
    };
    client
        .create_branch_ruleset(&token, "acme", "app", &spec)
        .await?;
    let rulesets = fake.rulesets("acme", "app");
    assert_eq!(rulesets.len(), 1);
    let ruleset = &rulesets[0];
    assert_eq!(ruleset["enforcement"], "active");
    assert_eq!(
        ruleset["conditions"]["ref_name"]["include"][0],
        "refs/heads/dev"
    );
    let types: Vec<&str> = ruleset["rules"]
        .as_array()
        .ok_or("rules")?
        .iter()
        .filter_map(|r| r["type"].as_str())
        .collect();
    assert_eq!(types, ["pull_request", "non_fast_forward", "deletion"]);
    assert_eq!(
        ruleset["rules"][0]["parameters"]["allowed_merge_methods"],
        serde_json::json!(["squash"])
    );
    // A writer cannot administer the repository.
    fake.add_user("dev", None);
    fake.set_permission("acme", "app", "dev", writer());
    let weak = fake.issue_user_token("dev");
    assert!(matches!(
        client
            .create_branch_ruleset(&weak, "acme", "app", &spec)
            .await,
        Err(GitHubError::Forbidden { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn pull_request_merge_refuses_stale_head() -> TestResult {
    let (fake, client) = setup().await?;
    let root = fake.add_repo("acme", "app", true);
    fake.set_branch("acme", "app", "dev", &root);
    let task_commit = fake.add_commit("acme", "app", &[&root]);
    fake.set_branch("acme", "app", "task/login", &task_commit);
    fake.add_user("dev", None);
    fake.set_permission("acme", "app", "dev", writer());
    let token = fake.issue_user_token("dev");
    let new_pull = NewPull {
        head: "task/login".into(),
        base: "dev".into(),
        title: "Add login".into(),
        body: "Implements the login spec.".into(),
    };
    let pull = client.create_pull(&token, "acme", "app", &new_pull).await?;
    assert_eq!(
        (pull.state, pull.head_sha.as_str()),
        (PullState::Open, task_commit.as_str())
    );
    assert!(matches!(
        client.create_pull(&token, "acme", "app", &new_pull).await,
        Err(GitHubError::Invalid { .. })
    ));
    let listed = client
        .list_pulls(&token, "acme", "app", PullFilter::Open, Some("task/login"))
        .await?;
    assert_eq!(listed.len(), 1);
    assert!(
        client
            .list_pulls(&token, "acme", "app", PullFilter::Open, Some("task/other"))
            .await?
            .is_empty()
    );

    // The agent pushes again after review: the reviewed head is stale.
    let newer = fake.add_commit("acme", "app", &[&task_commit]);
    fake.set_branch("acme", "app", "task/login", &newer);
    let stale = client
        .merge_pull(
            &token,
            "acme",
            "app",
            pull.number,
            MergeMethod::Squash,
            &task_commit,
        )
        .await;
    assert!(
        matches!(stale, Err(GitHubError::Conflict { .. })),
        "{stale:?}"
    );
    assert_eq!(fake.branch_sha("acme", "app", "dev"), Some(root.clone()));

    let merged = client
        .merge_pull(
            &token,
            "acme",
            "app",
            pull.number,
            MergeMethod::Squash,
            &newer,
        )
        .await?;
    assert_eq!(
        fake.branch_sha("acme", "app", "dev"),
        Some(merged.sha.clone())
    );
    let after = client.get_pull(&token, "acme", "app", pull.number).await?;
    assert_eq!(after.state, PullState::Merged);
    assert_eq!(after.merge_commit_sha, Some(merged.sha));
    assert!(matches!(
        client
            .merge_pull(
                &token,
                "acme",
                "app",
                pull.number,
                MergeMethod::Squash,
                &newer
            )
            .await,
        Err(GitHubError::NotMergeable { .. })
    ));

    // GitHub refuses to merge (e.g. required checks): typed NotMergeable.
    let second = fake.add_commit("acme", "app", &[&root]);
    fake.set_branch("acme", "app", "task/b", &second);
    let pull_b = client
        .create_pull(
            &token,
            "acme",
            "app",
            &NewPull {
                head: "task/b".into(),
                ..new_pull.clone()
            },
        )
        .await?;
    fake.set_pull_mergeable("acme", "app", pull_b.number, false);
    assert!(matches!(
        client
            .merge_pull(
                &token,
                "acme",
                "app",
                pull_b.number,
                MergeMethod::Merge,
                &second
            )
            .await,
        Err(GitHubError::NotMergeable { .. })
    ));
    let all = client
        .list_pulls(&token, "acme", "app", PullFilter::All, None)
        .await?;
    assert_eq!(all.len(), 2);
    Ok(())
}

#[tokio::test]
async fn fast_forward_is_refused_unless_descendant_and_compare_counts() -> TestResult {
    let (fake, client) = setup().await?;
    let root = fake.add_repo("acme", "app", true);
    let a = fake.add_commit("acme", "app", &[&root]);
    let b = fake.add_commit("acme", "app", &[&a]);
    let side = fake.add_commit("acme", "app", &[&root]);
    fake.set_branch("acme", "app", "quartio", &b);
    fake.set_branch("acme", "app", "feat/side", &side);
    fake.add_user("dev", None);
    fake.set_permission("acme", "app", "dev", writer());
    let token = fake.issue_user_token("dev");

    let compare = client
        .compare(&token, "acme", "app", "main", "quartio")
        .await?;
    assert_eq!(
        (compare.ahead_by, compare.behind_by, compare.status),
        (2, 0, CompareStatus::Ahead)
    );
    let diverged = client
        .compare(&token, "acme", "app", "quartio", "feat/side")
        .await?;
    assert_eq!((diverged.ahead_by, diverged.behind_by), (1, 2));
    assert_eq!(diverged.status, CompareStatus::Diverged);
    let behind = client.compare(&token, "acme", "app", &b, &root).await?;
    assert_eq!(behind.status, CompareStatus::Behind);

    assert_eq!(
        client
            .fast_forward_ref(&token, "acme", "app", "main", &side)
            .await,
        Ok(())
    );
    assert_eq!(fake.branch_sha("acme", "app", "main"), Some(side.clone()));
    assert_eq!(
        client
            .fast_forward_ref(&token, "acme", "app", "main", &b)
            .await,
        Err(GitHubError::NotFastForward)
    );
    assert_eq!(fake.branch_sha("acme", "app", "main"), Some(side));
    assert!(matches!(
        client
            .fast_forward_ref(&token, "acme", "app", "missing", &b)
            .await,
        Err(GitHubError::Invalid { .. })
    ));
    let identical = client
        .compare(&token, "acme", "app", "main", "feat/side")
        .await?;
    assert_eq!(identical.status, CompareStatus::Identical);
    let request = fake
        .requests()
        .into_iter()
        .find(|r| r.method == "PATCH")
        .ok_or("no PATCH")?;
    assert_eq!(request.path, "/repos/acme/app/git/refs/heads/main");
    assert!(request.body.contains("\"force\":false"));
    Ok(())
}

#[tokio::test]
async fn check_runs_and_statuses_aggregate() -> TestResult {
    let (fake, client) = setup().await?;
    let root = fake.add_repo("acme", "app", true);
    fake.add_user("dev", None);
    fake.set_permission("acme", "app", "dev", writer());
    let token = fake.issue_user_token("dev");
    let check = |sha: String| {
        let client = client.clone();
        let token = token.clone();
        async move {
            client
                .check_status(&token, "acme", "app", &sha)
                .await
                .map(|report| {
                    assert!(report.unreadable.is_empty(), "{report:?}");
                    report.summary
                })
        }
    };
    assert_eq!(check(root.clone()).await?, ChecksSummary::None);

    let pending = fake.add_commit("acme", "app", &[&root]);
    fake.set_check_runs(
        "acme",
        "app",
        &pending,
        vec![
            FakeCheckRun::new("test", "in_progress", None),
            FakeCheckRun::new("lint", "completed", Some("success")),
        ],
    );
    assert_eq!(check(pending).await?, ChecksSummary::Pending);

    let green = fake.add_commit("acme", "app", &[&root]);
    fake.set_check_runs(
        "acme",
        "app",
        &green,
        vec![
            FakeCheckRun::new("test", "completed", Some("success")),
            FakeCheckRun::new("docs", "completed", Some("skipped")),
        ],
    );
    fake.set_statuses(
        "acme",
        "app",
        &green,
        vec![FakeStatus::new("ci/legacy", "success")],
    );
    assert_eq!(check(green).await?, ChecksSummary::Success);

    let red = fake.add_commit("acme", "app", &[&root]);
    fake.set_check_runs(
        "acme",
        "app",
        &red,
        vec![
            FakeCheckRun::new("test", "completed", Some("failure")),
            FakeCheckRun::new("lint", "queued", None),
        ],
    );
    assert_eq!(check(red).await?, ChecksSummary::Failure);

    let status_only = fake.add_commit("acme", "app", &[&root]);
    fake.set_statuses(
        "acme",
        "app",
        &status_only,
        vec![
            FakeStatus::new("ci/a", "success"),
            FakeStatus::new("ci/b", "error"),
        ],
    );
    assert_eq!(check(status_only).await?, ChecksSummary::Failure);
    assert!(matches!(
        client.check_status(&token, "acme", "app", "main").await,
        Err(GitHubError::InvalidInput { .. })
    ));
    Ok(())
}

/// #111 (`F 2026-10-10`): an App without one of the check permissions still reads what it may.
/// GitHub answers 403 "Resource not accessible by integration" for the missing one.
#[tokio::test]
async fn checks_fall_back_to_the_sources_the_app_may_read() -> TestResult {
    let (fake, client) = setup().await?;
    let root = fake.add_repo("acme", "app", true);
    fake.add_user("dev", None);
    fake.set_permission("acme", "app", "dev", writer());
    let token = fake.issue_user_token("dev");
    let head = fake.add_commit("acme", "app", &[&root]);
    fake.set_check_runs(
        "acme",
        "app",
        &head,
        vec![FakeCheckRun::new("test", "completed", Some("success"))],
    );
    fake.set_statuses(
        "acme",
        "app",
        &head,
        vec![FakeStatus::new("deploy/preview", "success")],
    );
    fake.set_workflow_runs(
        "acme",
        "app",
        &head,
        vec![FakeCheckRun::new("CI", "in_progress", None)],
    );
    let check_runs = format!("/repos/acme/app/commits/{head}/check-runs");
    let status = format!("/repos/acme/app/commits/{head}/status");

    // 403 on check runs, 200 on statuses: statuses and the Actions runs decide (one is running).
    fake.inject_fault(
        FakeFault::new("GET", check_runs.clone(), 403, 1)
            .message("Resource not accessible by integration"),
    );
    let report = client.check_status(&token, "acme", "app", &head).await?;
    assert_eq!(report.summary, ChecksSummary::Pending);
    assert_eq!(report.unreadable, vec![CheckSource::CheckRuns]);
    assert_eq!(fake.count("GET", &status), 1);
    assert_eq!(fake.count("GET", "/repos/acme/app/actions/runs"), 1);
    let runs_request = fake
        .requests()
        .into_iter()
        .find(|r| r.path == "/repos/acme/app/actions/runs")
        .ok_or("no workflow runs request")?;
    assert!(
        runs_request
            .query
            .as_deref()
            .is_some_and(|q| q.contains(&format!("head_sha={head}"))),
        "{runs_request:?}"
    );

    // Without Actions either, the statuses alone count.
    fake.withhold_permission("checks");
    fake.withhold_permission("actions");
    let report = client.check_status(&token, "acme", "app", &head).await?;
    assert_eq!(report.summary, ChecksSummary::Success);
    assert_eq!(
        report.unreadable,
        vec![CheckSource::CheckRuns, CheckSource::WorkflowRuns]
    );

    // Nothing reported while sources are unreadable is unknown, not "no checks".
    let bare = fake.add_commit("acme", "app", &[&root]);
    let report = client.check_status(&token, "acme", "app", &bare).await?;
    assert_eq!(report.summary, ChecksSummary::Unknown);

    // Nothing readable at all: unknown, never green.
    fake.withhold_permission("statuses");
    let report = client.check_status(&token, "acme", "app", &head).await?;
    assert_eq!(report.summary, ChecksSummary::Unknown);
    assert_eq!(
        report.unreadable,
        vec![
            CheckSource::CheckRuns,
            CheckSource::CommitStatuses,
            CheckSource::WorkflowRuns
        ]
    );
    Ok(())
}

/// The installation of #111: Checks but no Commit statuses. Check runs decide; Actions runs are
/// not read (check runs include them); any other refusal still fails the read.
#[tokio::test]
async fn checks_without_commit_statuses_come_from_check_runs() -> TestResult {
    let (fake, client) = setup().await?;
    let root = fake.add_repo("acme", "app", true);
    fake.add_user("dev", None);
    fake.set_permission("acme", "app", "dev", writer());
    let token = fake.issue_user_token("dev");
    let head = fake.add_commit("acme", "app", &[&root]);
    fake.set_check_runs(
        "acme",
        "app",
        &head,
        vec![FakeCheckRun::new("test", "completed", Some("failure"))],
    );
    fake.withhold_permission("statuses");
    let report = client.check_status(&token, "acme", "app", &head).await?;
    assert_eq!(report.summary, ChecksSummary::Failure);
    assert_eq!(report.unreadable, vec![CheckSource::CommitStatuses]);
    assert_eq!(fake.count("GET", "/repos/acme/app/actions/runs"), 0);

    // A 403 for another reason (here: SAML enforcement) is not a missing permission.
    fake.inject_fault(
        FakeFault::new(
            "GET",
            format!("/repos/acme/app/commits/{head}/check-runs"),
            403,
            1,
        )
        .message("Resource protected by organization SAML enforcement."),
    );
    assert!(matches!(
        client.check_status(&token, "acme", "app", &head).await,
        Err(GitHubError::Forbidden { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn get_retries_transient_failures_but_mutations_do_not() -> TestResult {
    let (fake, client) = setup().await?;
    fake.add_repo("acme", "app", true);
    fake.add_user("dev", None);
    fake.set_permission("acme", "app", "dev", writer());
    let token = fake.issue_user_token("dev");

    fake.inject_fault(FakeFault::new("GET", "/repos/acme/app", 503, 1));
    let repo = client.get_repo(&token, "acme", "app").await?;
    assert_eq!(repo.name, "app");
    assert_eq!(fake.count("GET", "/repos/acme/app"), 2);

    // Exhaustion: three attempts, then the typed upstream error.
    fake.clear_requests();
    fake.inject_fault(FakeFault::new("GET", "/repos/acme/app", 502, 5));
    assert_eq!(
        client.get_repo(&token, "acme", "app").await,
        Err(GitHubError::Upstream(502))
    );
    assert_eq!(fake.count("GET", "/repos/acme/app"), 3);
    fake.clear_requests();
    for _ in 0..2 {
        // Drain the two remaining injected failures.
        let _ = client.get_repo(&token, "acme", "app").await;
    }

    // Secondary rate limit with Retry-After: 0 is retried.
    fake.clear_requests();
    fake.inject_fault(
        FakeFault::new("GET", "/repos/acme/app", 403, 1)
            .header("retry-after", "0")
            .message("You have exceeded a secondary rate limit"),
    );
    client.get_repo(&token, "acme", "app").await?;
    assert_eq!(fake.count("GET", "/repos/acme/app"), 2);

    // A reset far beyond the 5 s budget is surfaced immediately instead of waited out.
    fake.clear_requests();
    let reset = SystemClock.now().0 / 1000 + 3600;
    fake.inject_fault(
        FakeFault::new("GET", "/repos/acme/app", 403, 1)
            .header("x-ratelimit-remaining", "0")
            .header("x-ratelimit-reset", &reset.to_string()),
    );
    let started = Instant::now();
    match client.get_repo(&token, "acme", "app").await {
        Err(GitHubError::RateLimited {
            retry_after: Some(wait),
        }) => {
            assert!(wait > Duration::from_secs(3000));
        }
        other => return Err(format!("expected rate limit, got {other:?}").into()),
    }
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(fake.count("GET", "/repos/acme/app"), 1);

    // POST is never retried.
    fake.clear_requests();
    let root = fake.branch_sha("acme", "app", "main").ok_or("main")?;
    let head = fake.add_commit("acme", "app", &[&root]);
    fake.set_branch("acme", "app", "task/x", &head);
    fake.inject_fault(FakeFault::new("POST", "/repos/acme/app/pulls", 503, 1));
    let new_pull = NewPull {
        head: "task/x".into(),
        base: "main".into(),
        title: "x".into(),
        body: String::new(),
    };
    assert_eq!(
        client.create_pull(&token, "acme", "app", &new_pull).await,
        Err(GitHubError::Upstream(503))
    );
    assert_eq!(fake.count("POST", "/repos/acme/app/pulls"), 1);
    Ok(())
}

#[tokio::test]
async fn retry_budget_bounds_total_time() -> TestResult {
    let fake = FakeGitHub::start().await?;
    fake.add_repo("acme", "app", true);
    fake.add_user("dev", None);
    fake.set_permission("acme", "app", "dev", writer());
    let token = fake.issue_user_token("dev");
    let config = GitHubConfig {
        retry: RetryPolicy {
            max_attempts: 3,
            max_total: Duration::from_millis(300),
            base_delay: Duration::from_millis(200),
        },
        ..fake.config()
    };
    let client = GitHubClient::new(config)?;
    fake.inject_fault(FakeFault::new("GET", "/repos/acme/app", 500, 3));
    let started = Instant::now();
    assert_eq!(
        client.get_repo(&token, "acme", "app").await,
        Err(GitHubError::Upstream(500))
    );
    // 200 ms then 400 ms backoff would exceed the 300 ms budget: only two attempts.
    assert_eq!(fake.count("GET", "/repos/acme/app"), 2);
    assert!(started.elapsed() < Duration::from_secs(3));
    Ok(())
}

#[tokio::test]
async fn errors_and_debug_output_never_contain_tokens() -> TestResult {
    let (fake, client) = setup().await?;
    fake.add_repo("acme", "app", true);
    fake.add_user("dev", None);
    fake.set_permission("acme", "app", "dev", writer());
    let token = fake.issue_user_token("dev");
    let raw = token.expose().to_owned();

    // An upstream message that echoes the credential back.
    fake.inject_fault(
        FakeFault::new("GET", "/repos/acme/app", 403, 1).message(format!(
            "token {raw} lacks scope; also ghs_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        )),
    );
    let error = client
        .get_repo(&token, "acme", "app")
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(error, GitHubError::Forbidden { .. }));
    for text in [error.to_string(), format!("{error:?}")] {
        assert!(!text.contains(&raw), "{text}");
        assert!(!text.contains("ghs_AAAA"), "{text}");
    }

    // Transport failure against a closed port.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.local_addr()?.port()
    };
    let config = GitHubConfig {
        api_base: Url::parse(&format!("http://127.0.0.1:{closed}/api/"))?,
        retry: RetryPolicy {
            max_attempts: 1,
            ..RetryPolicy::default()
        },
        ..fake.config()
    };
    let offline = GitHubClient::new(config)?;
    let error = offline
        .get_repo(&token, "acme", "app")
        .await
        .err()
        .ok_or("expected a transport error")?;
    assert!(matches!(error, GitHubError::Transport(_)), "{error:?}");
    assert!(!format!("{error} {error:?}").contains(&raw));

    // Debug of token-bearing values and the client is redacted.
    let installation = fake.add_installation("acme", OwnerKind::Organization);
    let installation_token = client.create_installation_token(installation).await?;
    let debug = format!(
        "{installation_token:?} {client:?} {:?}",
        fake.app_credentials()
    );
    assert!(
        !debug.contains(installation_token.token.expose()),
        "{debug}"
    );
    assert!(!debug.contains("BEGIN RSA PRIVATE KEY"), "{debug}");
    assert!(!debug.contains("fake-client-secret"), "{debug}");

    // OAuth errors do not echo the code or client secret.
    let error = client
        .exchange_code("secret-code-xyz", "https://x/cb")
        .await
        .err()
        .ok_or("expected oauth error")?;
    assert!(!format!("{error:?}").contains("secret-code-xyz"));
    Ok(())
}

#[tokio::test]
async fn every_request_sends_the_required_headers() -> TestResult {
    let (fake, client) = setup().await?;
    fake.add_repo("acme", "app", true);
    fake.add_user("dev", None);
    fake.set_permission("acme", "app", "dev", writer());
    let token = fake.issue_user_token("dev");
    fake.add_oauth_code("c1", "dev");
    let installation = fake.add_installation("acme", OwnerKind::Organization);
    client.get_repo(&token, "acme", "app").await?;
    client.exchange_code("c1", "https://x/cb").await?;
    client.create_installation_token(installation).await?;
    client.find_installation("acme").await?;
    let requests = fake.requests();
    assert!(requests.len() >= 4);
    for request in requests {
        assert_eq!(
            request.header("user-agent"),
            Some("yulius"),
            "{}",
            request.path
        );
        assert_eq!(
            request.header("x-github-api-version"),
            Some("2022-11-28"),
            "{}",
            request.path
        );
        let expected_accept = if request.web {
            "application/json"
        } else {
            "application/vnd.github+json"
        };
        assert_eq!(
            request.header("accept"),
            Some(expected_accept),
            "{}",
            request.path
        );
    }
    Ok(())
}

#[tokio::test]
async fn repository_tokens_are_restricted_and_cached_per_repository() -> TestResult {
    let fake = FakeGitHub::start().await?;
    let installation = fake.add_installation("acme", OwnerKind::Organization);
    let client = GitHubClient::new(fake.config())?;
    let path = format!("/app/installations/{installation}/access_tokens");

    let scoped = client.create_repository_token(installation, "app").await?;
    let again = client.create_repository_token(installation, "APP").await?;
    assert_eq!(scoped, again, "cached per repository, case-insensitively");
    let unrestricted = client.create_installation_token(installation).await?;
    assert_ne!(scoped.token, unrestricted.token, "separate cache entries");
    assert_eq!(fake.count("POST", &path), 2);
    let bodies: Vec<serde_json::Value> = fake
        .requests()
        .into_iter()
        .filter(|r| r.path == path)
        .map(|r| serde_json::from_str(&r.body))
        .collect::<Result<_, _>>()?;
    assert_eq!(bodies[0]["repositories"], serde_json::json!(["app"]));
    assert!(bodies[1].get("repositories").is_none());
    assert!(matches!(
        client.create_repository_token(installation, "../x").await,
        Err(GitHubError::InvalidInput { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn commit_statuses_are_published_and_branches_deleted() -> TestResult {
    let (fake, client) = setup().await?;
    let root = fake.add_repo("acme", "app", true);
    fake.set_branch("acme", "app", "dev", &root);
    let head = fake.add_commit("acme", "app", &[&root]);
    fake.set_branch("acme", "app", "task/ci", &head);
    fake.add_user("dev", None);
    fake.set_permission("acme", "app", "dev", writer());
    let token = fake.issue_user_token("dev");
    let status = |state: &str| CommitStatus {
        state: state.into(),
        context: "yulius/ci".into(),
        description: format!("{state} {}", "x".repeat(300)),
        target_url: Some("https://yulius.example.com/s/app/ci?run=r1".into()),
    };
    client
        .create_commit_status(&token, "acme", "app", &head, &status("pending"))
        .await?;
    let report = client.check_status(&token, "acme", "app", &head).await?;
    assert_eq!(report.summary, ChecksSummary::Pending);
    client
        .create_commit_status(&token, "acme", "app", &head, &status("success"))
        .await?;
    let report = client.check_status(&token, "acme", "app", &head).await?;
    assert_eq!(report.summary, ChecksSummary::Success);
    let sent = fake
        .requests()
        .into_iter()
        .rfind(|r| r.method == "POST" && r.path.ends_with(&format!("/statuses/{head}")))
        .ok_or("status request")?;
    let body: serde_json::Value = serde_json::from_str(&sent.body)?;
    assert_eq!(body["context"], "yulius/ci");
    assert_eq!(body["description"].as_str().map(|d| d.chars().count()), Some(140));
    // Invalid input never reaches GitHub.
    assert!(matches!(
        client
            .create_commit_status(&token, "acme", "app", &head, &status("green"))
            .await,
        Err(GitHubError::InvalidInput { .. })
    ));
    // Without "Commit statuses: write" GitHub answers 403.
    fake.withhold_permission("statuses");
    assert!(matches!(
        client
            .create_commit_status(&token, "acme", "app", &head, &status("success"))
            .await,
        Err(GitHubError::Forbidden { .. })
    ));
    // Branch deletion: true once, false when already gone.
    assert!(client.delete_branch(&token, "acme", "app", "task/ci").await?);
    assert!(!client.delete_branch(&token, "acme", "app", "task/ci").await?);
    assert_eq!(fake.branch_sha("acme", "app", "task/ci"), None);
    Ok(())
}
