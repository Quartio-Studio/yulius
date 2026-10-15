//! SPEC-141 (issue #486, ADR-0114): only a person moves the production branch. The production
//! GitHub adapter over the in-process fake GitHub, which models GitHub's rulesets (`update` and
//! `deletion` rules, bypass actors) and narrowed installation tokens: the token an agent's Git and
//! `gh` hold can neither merge into, fast-forward nor delete the production branch Yulius protects,
//! nor remove that protection; a repository admin still promotes. No real GitHub call.

use yulius::wiring::github_client::GitHubAdapter;
use yulius_app::model::Secret;
use yulius_app::ports::github::{GitHubAppConfig, GitHubPort, NewPull};
use yulius_app::projects::rulesets_for;
use yulius_domain::delivery::DeliveryProfile;
use yulius_github::fake::FakeGitHub;
use yulius_github::{GitHubPermissions, OwnerKind};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A repository `acme/app` with `main` at its root commit and `dev` one commit ahead, an
/// installation of the App on `acme`, a repository admin `boss` and the adapter.
struct World {
    fake: FakeGitHub,
    adapter: GitHubAdapter,
    app: GitHubAppConfig,
    installation: u64,
    admin: Secret,
    root: String,
    dev: String,
}

async fn world() -> Result<World, Box<dyn std::error::Error>> {
    let fake = FakeGitHub::start().await?;
    let adapter = GitHubAdapter::new(fake.config())?;
    let installation = fake.add_installation("acme", OwnerKind::Organization);
    let root = fake.add_repo("acme", "app", true);
    let dev = fake.add_commit("acme", "app", &[&root]);
    fake.set_branch("acme", "app", "dev", &dev);
    fake.add_user("boss", None);
    let admin = GitHubPermissions {
        admin: true,
        maintain: true,
        push: true,
        triage: true,
        pull: true,
    };
    fake.set_permission("acme", "app", "boss", admin);
    let admin = Secret(fake.issue_user_token("boss").expose().to_owned());
    let credentials = fake.app_credentials();
    let app = GitHubAppConfig {
        app_id: credentials.app_id,
        slug: credentials.slug.clone(),
        client_id: credentials.client_id.clone(),
        client_secret: credentials.client_secret.expose().to_owned(),
        private_key_pem: credentials.private_key_pem.expose().to_owned(),
        webhook_secret: None,
    };
    Ok(World {
        fake,
        adapter,
        app,
        installation,
        admin,
        root,
        dev,
    })
}

impl World {
    /// The token the credential helper hands an agent run (`App::agent_git_credential`).
    async fn agent_token(&self) -> Result<Secret, Box<dyn std::error::Error>> {
        Ok(self
            .adapter
            .agent_token(&self.app, self.installation, "app")
            .await?)
    }

    /// The dev → main promotion pull request, opened by `token`.
    async fn promotion(&self, token: &Secret) -> Result<u64, Box<dyn std::error::Error>> {
        let pull = NewPull {
            head: "dev".into(),
            base: "main".into(),
            title: "Release".into(),
            body: String::new(),
        };
        Ok(self
            .adapter
            .create_pull(token, "acme", "app", &pull)
            .await?
            .number)
    }

    fn main(&self) -> Option<String> {
        self.fake.branch_sha("acme", "app", "main")
    }
}

/// AC2: with the rulesets Yulius gives a new project, the agent's token is refused every way of
/// moving `main` (merge, fast-forward, deletion) and cannot delete the protection itself; the
/// repository admin's promotion goes through.
#[tokio::test]
async fn an_agents_push_or_merge_into_production_is_refused() -> TestResult {
    let w = world().await?;
    for ruleset in rulesets_for(&DeliveryProfile::standard()) {
        w.adapter
            .create_ruleset(&w.admin, "acme", "app", &ruleset)
            .await?;
    }
    let agent = w.agent_token().await?;
    assert!(
        w.adapter
            .branch_guard(&agent, "acme", "app", "main")
            .await?
            .is_guarded()
    );

    // Opening a promotion pull request is harmless; merging it is not the agent's to do.
    let number = w.promotion(&agent).await?;
    let merged = w
        .adapter
        .merge_pull(&agent, "acme", "app", number, "merge", &w.dev)
        .await;
    assert!(merged.is_err(), "{merged:?}");
    let forwarded = w
        .adapter
        .fast_forward(&agent, "acme", "app", "main", &w.dev)
        .await;
    assert!(forwarded.is_err(), "{forwarded:?}");
    let deleted = w.adapter.delete_branch(&agent, "acme", "app", "main").await;
    assert!(deleted.is_err(), "{deleted:?}");
    assert_eq!(w.main(), Some(w.root.clone()), "main never moved");

    // The protection cannot be removed with the agent's token (no Administration permission).
    let rulesets = w.fake.rulesets("acme", "app");
    let production = rulesets
        .iter()
        .find(|r| r["name"] == "yulius-production-main")
        .ok_or("production ruleset")?;
    let url = format!(
        "{}/repos/acme/app/rulesets/{}",
        w.fake.config().api_base.as_str().trim_end_matches('/'),
        production["id"]
    );
    let removal = reqwest::Client::new()
        .delete(&url)
        .bearer_auth(agent.expose())
        .send()
        .await?;
    assert_eq!(removal.status(), 403);
    assert_eq!(w.fake.rulesets("acme", "app").len(), 2);

    // Its own task branches stay the agent's to move.
    let task = w.fake.add_commit("acme", "app", &[&w.dev]);
    w.fake.set_branch("acme", "app", "feat/1-x", &w.dev);
    w.adapter
        .fast_forward(&agent, "acme", "app", "feat/1-x", &task)
        .await?;

    // A person (a repository admin) promotes.
    let promoted = w
        .adapter
        .merge_pull(&w.admin, "acme", "app", number, "merge", &w.dev)
        .await?;
    assert_eq!(w.main(), Some(promoted));
    Ok(())
}

/// Without the production ruleset (an imported repository nobody protected), GitHub lets the very
/// same token merge into `main`: this is what onboarding reports, and why a person must add it.
#[tokio::test]
async fn without_the_ruleset_the_agents_token_could_write_production() -> TestResult {
    let w = world().await?;
    let agent = w.agent_token().await?;
    let guard = w
        .adapter
        .branch_guard(&agent, "acme", "app", "main")
        .await?;
    assert!(!guard.is_guarded(), "{guard:?}");
    let number = w.promotion(&agent).await?;
    w.adapter
        .merge_pull(&agent, "acme", "app", number, "merge", &w.dev)
        .await?;
    assert_ne!(w.main(), Some(w.root.clone()));
    Ok(())
}
