//! Shared fixtures moved from `delivery`.

use crate::support;
use crate::support::BareRemote;
use crate::support::git;
use async_trait::async_trait;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use support::{Creds, Resp, TestResult, call};
use yulius_app::agent_wait::WaitState;
use yulius_app::delivery::profile_to_contract;
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::{
    AttemptRecord, GitHubTokenRecord, LaunchKind, SandboxRecord, Sealed, Secret, StepRecord,
    TaskOrigin, TaskRecord, UserRecord,
};
use yulius_app::ports::browser::{BrowserFailure, BrowserPort, BrowserSession};
use yulius_app::ports::git::{CommitInfo, DiffStat, GitPort, MergeIntegration};
use yulius_app::ports::github::{ChecksReport, ChecksSource, ChecksState, GhPull};
use yulius_app::ports::session::{Containment, SessionHost};
use yulius_app::ports::store::{
    AuditStore, CoordinationStore, InstanceStore, SandboxStore, TaskStore, UserStore,
};
use yulius_app::ports::store_ext::DeliveryStore;
use yulius_domain::delivery::{DeliveryProfile, DeliveryStage};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, SandboxId, StepId, TaskId, UserId};
use yulius_domain::permission::GitHubPermissions;
use yulius_domain::task::{Activity, AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

/// Chromium startup can hold the final lookup for outstanding browser decisions.
#[derive(Default)]
pub(crate) struct HeldBrowserLookup {
    session: Option<Arc<dyn BrowserSession>>,
    publication_reader: Option<Arc<std::sync::Mutex<Option<tokio::task::Id>>>>,
    pub(crate) calls: std::sync::atomic::AtomicUsize,
    pub(crate) entered: Arc<tokio::sync::Notify>,
    pub(crate) release: Arc<tokio::sync::Notify>,
}

impl HeldBrowserLookup {
    pub(crate) fn with_session(session: Arc<dyn BrowserSession>) -> Self {
        Self {
            session: Some(session),
            ..Self::default()
        }
    }

    pub(crate) fn for_publication() -> Self {
        Self {
            publication_reader: Some(Arc::default()),
            ..Self::default()
        }
    }

    pub(crate) fn hold_lookup_for(
        &self,
        reader: tokio::task::Id,
    ) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
        if let Some(target) = &self.publication_reader {
            *target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(reader);
        }
        (self.entered.clone(), self.release.clone())
    }

    pub(crate) fn hold_publication_lookup(
        &self,
        task: &TaskId,
    ) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
        if let Some(reader) = self.publication_reader.clone() {
            yulius_app::race::on(
                yulius_app::race::Point::ArmedNoticePublicationRead,
                Path::new(task.as_str()),
                move || {
                    *reader
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(tokio::task::id());
                },
            );
        }
        (self.entered.clone(), self.release.clone())
    }
}

#[async_trait]
impl BrowserPort for HeldBrowserLookup {
    async fn ensure(
        &self,
        sandbox: &SandboxId,
        profile_dir: &Path,
    ) -> Result<Arc<dyn BrowserSession>, BrowserFailure> {
        yulius::wiring::browser::UnavailableBrowser
            .ensure(sandbox, profile_dir)
            .await
    }

    async fn restart(
        &self,
        sandbox: &SandboxId,
        profile_dir: &Path,
    ) -> Result<Arc<dyn BrowserSession>, BrowserFailure> {
        yulius::wiring::browser::UnavailableBrowser
            .restart(sandbox, profile_dir)
            .await
    }

    async fn running(&self, _: &SandboxId) -> Option<Arc<dyn BrowserSession>> {
        let hold = match &self.publication_reader {
            Some(reader) => reader
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take_if(|watcher| Some(*watcher) == tokio::task::try_id())
                .is_some(),
            None => self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1,
        };
        if hold {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.session.clone()
    }

    async fn reap_idle(&self) -> Vec<SandboxId> {
        Vec::new()
    }

    async fn stop(&self, _: &SandboxId) -> bool {
        false
    }

    async fn shutdown_all(&self) -> Vec<SandboxId> {
        Vec::new()
    }
}
