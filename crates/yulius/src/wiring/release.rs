//! [`ReleaseSource`] over the GitHub REST API (SPEC-002 ADR-18): the head of `main` of
//! Quartio-Studio/yulius and the commits from the running build to it.
//!
//! `GET /repos/{owner}/{repo}/commits/main` gives the head; `GET …/compare/{build}...{head}`
//! says whether the build is behind, ahead, diverged or identical and lists the commits between
//! (GitHub returns up to 250, oldest first). An unknown build (or one GitHub does not know) gets
//! the latest commits of `main` instead. One deadline per request, no redirects, bounded bodies;
//! errors never carry the token or the body.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use yulius_app::model::Secret;
use yulius_app::ports::updates::{ReleaseSource, ReleaseState, RemoteCommit, SourceError};
use yulius_domain::update::{BRANCH, REPO_NAME, REPO_OWNER, Relation, is_commit};

/// GitHub's API.
pub const GITHUB_API: &str = "https://api.github.com";
const TIMEOUT: Duration = Duration::from_secs(15);
/// Largest answer read (a compare of 250 commits is about 1 MiB).
const MAX_BODY: usize = 4 * 1024 * 1024;
/// Commits kept from one answer.
const MAX_COMMITS: usize = 250;

/// `main` of the update repository on GitHub.
pub struct GitHubReleases {
    http: reqwest::Client,
    api: String,
}

impl GitHubReleases {
    /// A source for github.com.
    ///
    /// # Errors
    /// When the HTTP client cannot be built.
    pub fn new() -> Result<Self, String> {
        Self::with_api(GITHUB_API.to_owned(), true)
    }

    /// A source for another API base (tests).
    ///
    /// # Errors
    /// When the HTTP client cannot be built.
    pub fn with_api(api: String, https_only: bool) -> Result<Self, String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(concat!("yulius/", env!("CARGO_PKG_VERSION")))
            .timeout(TIMEOUT)
            .https_only(https_only)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("GitHub client setup failed: {e}"))?;
        Ok(Self {
            http,
            api: api.trim_end_matches('/').to_owned(),
        })
    }

    async fn get(&self, token: Option<&Secret>, path: &str) -> Result<Option<Value>, SourceError> {
        let mut request = self
            .http
            .get(format!("{}/repos/{REPO_OWNER}/{REPO_NAME}{path}", self.api))
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = token {
            request = request.bearer_auth(token.expose());
        }
        let mut response = request.send().await.map_err(|_| SourceError::Unavailable)?;
        let status = response.status();
        let limited = response
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            == Some("0");
        match status.as_u16() {
            200 => {}
            // A commit GitHub does not know (a local build); the caller decides.
            404 if path.contains("/compare/") => return Ok(None),
            403 | 429 if limited || status.as_u16() == 429 => return Err(SourceError::RateLimited),
            401 | 403 | 404 => return Err(SourceError::NoAccess),
            _ => return Err(SourceError::Unavailable),
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| SourceError::Unavailable)?
        {
            if bytes.len() + chunk.len() > MAX_BODY {
                return Err(SourceError::Invalid("body"));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| SourceError::Invalid("json"))
    }
}

/// One commit object of GitHub's commit list or compare answer.
fn commit_of(value: &Value) -> Option<RemoteCommit> {
    let sha = value.get("sha")?.as_str().filter(|s| is_commit(s))?;
    let commit = value.get("commit")?;
    Some(RemoteCommit {
        sha: sha.to_owned(),
        message: commit
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .chars()
            .take(4_000)
            .collect(),
        author: value
            .get("author")
            .and_then(|a| a.get("login"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        date: commit
            .get("committer")
            .and_then(|c| c.get("date"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// The compare answer from the build to the head: relation and commits, newest first.
pub fn parse_compare(body: &Value) -> Result<(Relation, Vec<RemoteCommit>), SourceError> {
    let relation = match body.get("status").and_then(Value::as_str) {
        // `ahead`: the head (main) is ahead of the base (the build).
        Some("ahead") => Relation::Behind,
        Some("behind") => Relation::Ahead,
        Some("identical") => Relation::UpToDate,
        Some("diverged") => Relation::Diverged,
        _ => return Err(SourceError::Invalid("status")),
    };
    let mut commits: Vec<RemoteCommit> = body
        .get("commits")
        .and_then(Value::as_array)
        .ok_or(SourceError::Invalid("commits"))?
        .iter()
        .filter_map(commit_of)
        .collect();
    commits.reverse();
    commits.truncate(MAX_COMMITS);
    Ok((relation, commits))
}

#[async_trait]
impl ReleaseSource for GitHubReleases {
    async fn compare(
        &self,
        token: Option<&Secret>,
        build: Option<&str>,
    ) -> Result<ReleaseState, SourceError> {
        let head = self
            .get(token, &format!("/commits/{BRANCH}"))
            .await?
            .ok_or(SourceError::NoAccess)?;
        let head = commit_of(&head).ok_or(SourceError::Invalid("head"))?;
        if let Some(build) = build.filter(|b| is_commit(b)) {
            if build == head.sha {
                return Ok(ReleaseState {
                    head: head.sha,
                    relation: Relation::UpToDate,
                    commits: Vec::new(),
                });
            }
            if let Some(body) = self
                .get(token, &format!("/compare/{build}...{}", head.sha))
                .await?
            {
                let (relation, commits) = parse_compare(&body)?;
                return Ok(ReleaseState {
                    head: head.sha,
                    relation,
                    commits,
                });
            }
        }
        // Unknown build: the latest commits of main.
        let list = self
            .get(token, &format!("/commits?sha={BRANCH}&per_page=20"))
            .await?
            .ok_or(SourceError::NoAccess)?;
        let commits = list
            .as_array()
            .ok_or(SourceError::Invalid("commits"))?
            .iter()
            .filter_map(commit_of)
            .collect();
        Ok(ReleaseState {
            head: head.sha,
            relation: Relation::Unknown,
            commits,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;
    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";
    const C: &str = "3333333333333333333333333333333333333333";

    fn commit(sha: &str, message: &str) -> Value {
        json!({"sha": sha, "commit": {"message": message, "committer": {"date": "2026-10-10T20:00:00Z"}},
               "author": {"login": "octocat"}})
    }

    /// A small HTTP server answering by path: (status, body), recording request heads.
    async fn server(
        routes: Vec<(String, u16, String)>,
    ) -> Result<(String, Arc<Mutex<Vec<String>>>), std::io::Error> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0u8; 16 * 1024];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                if let Ok(mut log) = log.lock() {
                    log.push(head.clone());
                }
                let (status, body) = routes
                    .iter()
                    .find(|(p, _, _)| path.ends_with(p))
                    .map_or((404, "{}".to_owned()), |(_, s, b)| (*s, b.clone()));
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        Ok((url, seen))
    }

    #[tokio::test]
    async fn a_build_behind_main_lists_the_new_commits_newest_first() -> TestResult {
        let compare = json!({"status": "ahead", "ahead_by": 2,
            "commits": [commit(B, "feat(a): first (#1)"), commit(C, "fix(b): second (#2)")]});
        let (url, seen) = server(vec![
            (
                "/commits/main".into(),
                200,
                commit(C, "fix(b): second (#2)").to_string(),
            ),
            (format!("/compare/{A}...{C}"), 200, compare.to_string()),
        ])
        .await?;
        let source = GitHubReleases::with_api(url, false)?;
        let state = source
            .compare(Some(&Secret("ghs_synthetic".into())), Some(A))
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(state.head, C);
        assert_eq!(state.relation, Relation::Behind);
        assert_eq!(
            state
                .commits
                .iter()
                .map(|c| c.sha.as_str())
                .collect::<Vec<_>>(),
            vec![C, B]
        );
        assert_eq!(state.commits[0].author.as_deref(), Some("octocat"));
        let heads = seen
            .lock()
            .map_err(|_| "lock")?
            .join("\n")
            .to_ascii_lowercase();
        assert!(heads.contains("authorization: bearer ghs_synthetic"));
        assert!(heads.contains("/repos/quartio-studio/yulius/commits/main"));
        Ok(())
    }

    #[tokio::test]
    async fn the_head_build_is_up_to_date_and_an_unknown_build_gets_the_latest_commits()
    -> TestResult {
        let (url, _) = server(vec![
            ("/commits/main".into(), 200, commit(C, "fix: c").to_string()),
            (
                "/commits?sha=main&per_page=20".into(),
                200,
                json!([commit(C, "fix: c"), commit(B, "feat: b")]).to_string(),
            ),
        ])
        .await?;
        let source = GitHubReleases::with_api(url, false)?;
        let same = source
            .compare(None, Some(C))
            .await
            .map_err(|e| e.to_string())?;
        assert_eq!(same.relation, Relation::UpToDate);
        assert!(same.commits.is_empty());
        // A build GitHub does not know (the compare answers 404) and no build at all.
        for build in [Some(A), None] {
            let state = source
                .compare(None, build)
                .await
                .map_err(|e| e.to_string())?;
            assert_eq!(state.relation, Relation::Unknown);
            assert_eq!(state.commits.len(), 2);
        }
        Ok(())
    }

    #[tokio::test]
    async fn refusals_and_rate_limits_are_typed() -> TestResult {
        let (url, _) = server(vec![("/commits/main".into(), 404, "{}".into())]).await?;
        let source = GitHubReleases::with_api(url, false)?;
        assert_eq!(source.compare(None, None).await, Err(SourceError::NoAccess));
        let (url, _) = server(vec![("/commits/main".into(), 429, "{}".into())]).await?;
        let source = GitHubReleases::with_api(url, false)?;
        assert_eq!(
            source.compare(None, None).await,
            Err(SourceError::RateLimited)
        );
        let (url, _) = server(vec![("/commits/main".into(), 500, "{}".into())]).await?;
        let source = GitHubReleases::with_api(url, false)?;
        assert_eq!(
            source.compare(None, None).await,
            Err(SourceError::Unavailable)
        );
        let (url, _) = server(vec![(
            "/commits/main".into(),
            200,
            "{\"sha\": \"nope\"}".into(),
        )])
        .await?;
        let source = GitHubReleases::with_api(url, false)?;
        assert_eq!(
            source.compare(None, None).await,
            Err(SourceError::Invalid("head"))
        );
        Ok(())
    }

    #[test]
    fn compare_statuses_map_to_the_build_side() -> Result<(), String> {
        for (status, relation) in [
            ("ahead", Relation::Behind),
            ("behind", Relation::Ahead),
            ("identical", Relation::UpToDate),
            ("diverged", Relation::Diverged),
        ] {
            let (got, _) = parse_compare(&json!({"status": status, "commits": []}))
                .map_err(|e| e.to_string())?;
            assert_eq!(got, relation);
        }
        assert!(parse_compare(&json!({"status": "?", "commits": []})).is_err());
        Ok(())
    }
}
