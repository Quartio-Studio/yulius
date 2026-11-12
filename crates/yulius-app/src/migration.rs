//! A move to another host asked from Settings → Instance (SPEC-470, ADR-0305): the request a
//! superadmin writes for the root helper, and what the page is told about it.
//!
//! Nothing here is stored. The server is frozen while its data is copied and stays frozen on
//! the old host after a move, and its database travels with the copy: a row "running" would
//! arrive on the new host as a move of that host. So every answer is computed from the files
//! the helper and the command keep outside the data directory, and from the request file, which
//! is removed once the helper took it.

pub mod rules;

use std::sync::Arc;

use yulius_contracts::api::migration::{
    AskMigration, MigrationArrived, MigrationCan, MigrationMove, MigrationRequest, MigrationRun,
    MigrationView,
};
use yulius_contracts::migrate::{
    MoveAction, MoveChecklist, MoveHelperState, MoveHelperStatus, MoveRequest, REQUEST_TTL_MS,
    VERSION, Values,
};
use yulius_contracts::update::HostOutcome;
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::migration::{MoveHelper, MoveStatus};

/// A request the helper has not taken after this long is said to be unanswered: its path unit
/// is not enabled, or it could not start.
pub const ACK_WAIT_MS: i64 = 20_000;
/// A request the helper has not taken after this long is written again when it is read, while
/// its answer is awaited: the path unit fires on a change of the file, and a change that comes
/// while the helper is ending another run is lost.
pub const RETRY_AFTER_MS: i64 = 5_000;
/// A running helper whose report is older than this stopped without a word.
pub const LOST_AFTER_MS: i64 = 60_000;
/// A move is asked on a plan shown at most this long ago.
pub const PLAN_VALID_MS: i64 = 3_600_000;
/// The SSH port when the person names none.
const SSH_PORT: u16 = 22;

const PLAN_FIRST: &str = "show the plan for these values first: a move is the plan you saw";
const NOT_ENABLED: &str = "the move helper is not enabled on this server (the operations guide \
                           has its units); the command `sudo yulius instance migrate` on the \
                           host does the same";

/// The helper's files as one read found them.
struct Files {
    installed: bool,
    pending: Option<MoveRequest>,
    helper: Option<MoveHelperStatus>,
    status: Option<MoveStatus>,
    arrived: Option<MoveChecklist>,
}

/// Where a move stands, from [`Files`] at one moment.
struct Facts {
    files: Files,
    now: i64,
    update_running: bool,
}

impl Facts {
    /// The request that waits: `Some(true)` while the helper may still take it, `Some(false)`
    /// once it is overdue.
    fn waiting(&self) -> Option<bool> {
        self.files
            .pending
            .as_ref()
            .map(|request| self.now - request.requested_at <= ACK_WAIT_MS)
    }

    /// The helper's report, with `true` when it says running and still reports.
    fn run(&self) -> Option<(&MoveHelperStatus, &'static str)> {
        let run = self.files.helper.as_ref()?;
        let state = match run.state {
            MoveHelperState::Running if self.now - run.updated_at > LOST_AFTER_MS => "lost",
            MoveHelperState::Running => "running",
            MoveHelperState::Ended => "ended",
            MoveHelperState::Refused => "refused",
        };
        Some((run, state))
    }

    /// Whether the helper runs a command now, and for which action.
    fn running(&self) -> Option<MoveAction> {
        self.run()
            .filter(|(_, state)| *state == "running")
            .map(|(run, _)| run.action)
    }

    /// Whether the checklist was written after the helper's last run ended (or without any):
    /// by a run started on the host.
    fn by_command(&self) -> bool {
        let Some(status) = &self.files.status else {
            return false;
        };
        match &self.files.helper {
            None => true,
            Some(run) => run.ended_at.is_some_and(|ended| {
                i64::try_from(status.updated_at)
                    .unwrap_or(i64::MAX)
                    .saturating_mul(1000)
                    > ended
            }),
        }
    }

    /// Whether a move of `values` may be asked: the plan the person saw, or a move that
    /// stopped. `None` compares with nothing (what the view offers).
    fn movable(&self, values: Option<&Values>) -> Result<(), &'static str> {
        let Some((run, state)) = self.run() else {
            return Err(PLAN_FIRST);
        };
        if self.by_command() || values.is_some_and(|values| *values != run.values) {
            return Err(PLAN_FIRST);
        }
        let status = self.files.status.as_ref();
        match (run.action, state) {
            (MoveAction::Plan, "ended") => {
                let failed = status.is_some_and(|status| {
                    status
                        .lines
                        .iter()
                        .any(|line| line.outcome == HostOutcome::Failed)
                });
                if failed {
                    return Err("the plan found problems: fix them and show the plan again");
                }
                let Some(status) = status.filter(|_| run.exit_code == Some(0)) else {
                    return Err(PLAN_FIRST);
                };
                if status.public_key.is_some() {
                    return Err(
                        "add the key to root's authorized_keys on the new host and show the \
                         plan again: this one ended before it reached the new host",
                    );
                }
                if run
                    .ended_at
                    .is_none_or(|ended| self.now - ended > PLAN_VALID_MS)
                {
                    return Err("the plan was shown more than an hour ago: show it again first");
                }
                Ok(())
            }
            // A move that stopped is continued; one that ended well has nothing left to do.
            (MoveAction::Move, "ended") if run.exit_code != Some(0) => Ok(()),
            (MoveAction::Move, "lost") => Ok(()),
            _ => Err(PLAN_FIRST),
        }
    }

    /// Whether something is recorded that an abort goes back from.
    fn recorded(&self) -> bool {
        let unfinished = self
            .files
            .status
            .as_ref()
            .is_some_and(|status| !matches!(status.phase.as_str(), "done" | "aborted"));
        let run = self.run().is_some_and(|(run, state)| match state {
            "running" => run.action != MoveAction::Abort,
            "lost" => true,
            "ended" => run.exit_code != Some(0),
            _ => false,
        });
        unfinished || run
    }

    fn abort_in_flight(&self) -> bool {
        let waits = self.waiting() == Some(true)
            && self
                .files
                .pending
                .as_ref()
                .is_some_and(|request| request.action == MoveAction::Abort);
        waits || self.running() == Some(MoveAction::Abort)
    }

    fn can(&self) -> MigrationCan {
        let open = self.files.installed && !self.update_running;
        let idle = self.waiting() != Some(true) && self.running().is_none();
        MigrationCan {
            plan: open && idle,
            moving: open && idle && self.movable(None).is_ok(),
            abort: open
                && self.waiting() != Some(true)
                && !self.abort_in_flight()
                && self.recorded(),
        }
    }

    fn view(&self, public_url: &str) -> MigrationView {
        let at = |ms: i64| rfc3339(Timestamp(ms));
        let by_command = self.by_command();
        MigrationView {
            public_url: public_url.to_owned(),
            helper_installed: self.files.installed,
            update_running: self.update_running,
            request: self.files.pending.as_ref().map(|request| MigrationRequest {
                id: request.id.clone(),
                action: request.action,
                state: if self.waiting() == Some(true) {
                    "requested"
                } else {
                    "unanswered"
                }
                .to_owned(),
                requested_at: at(request.requested_at),
            }),
            run: self.run().map(|(run, state)| MigrationRun {
                id: run.id.clone(),
                action: run.action,
                target: run.values.target.clone(),
                ssh_port: run.values.ssh_port,
                public_url: run.values.public_url.clone(),
                data_disks: run.values.data_disks.clone(),
                state: state.to_owned(),
                exit_code: run.exit_code,
                error: run.error.clone(),
                started_at: at(run.started_at),
                updated_at: at(run.updated_at),
                ended_at: run.ended_at.map(at),
            }),
            moving: self.files.status.as_ref().map(|status| MigrationMove {
                id: status.id.clone(),
                target: status.target.clone(),
                phase: status.phase.clone(),
                lines: status.lines.clone(),
                public_key: status.public_key.clone(),
                new_url: status.new_url.clone(),
                updated_at: at(i64::try_from(status.updated_at)
                    .unwrap_or(i64::MAX)
                    .saturating_mul(1000)),
                by_command,
            }),
            arrived: self.files.arrived.as_ref().map(|arrived| MigrationArrived {
                id: arrived.id.clone(),
                target: arrived.target.clone(),
                from: arrived.from.clone(),
                to: arrived.to.clone(),
                finished_at: at(i64::try_from(arrived.finished_at)
                    .unwrap_or(i64::MAX)
                    .saturating_mul(1000)),
                lines: arrived.lines.clone(),
            }),
            can: self.can(),
        }
    }
}

/// Reads the helper's files; a request the helper took, or one too old to be taken, is removed.
fn read(helper: &dyn MoveHelper, now: i64) -> Files {
    let report = helper.helper_status();
    let mut pending = helper.pending();
    if let Some(request) = &pending {
        let taken = report.as_ref().is_some_and(|run| run.id == request.id);
        let age = now - request.requested_at;
        // Never left for a copy to carry to another host as if it were asked there.
        if taken || age > REQUEST_TTL_MS {
            helper.clear_request();
            pending = None;
        } else if age > RETRY_AFTER_MS && age <= ACK_WAIT_MS {
            // The same request, with its id and its time: the helper acts on an id once.
            let _ = helper.request(request);
        }
    }
    Files {
        installed: helper.installed(),
        pending,
        helper: report,
        status: helper.status(),
        arrived: helper.arrived(),
    }
}

/// What a person typed, as the request holds it.
fn asked(req: &AskMigration) -> Values {
    if req.action == MoveAction::Abort {
        // An abort that names something is refused by the rules, not silently stripped.
        return Values {
            target: req.target.clone(),
            ssh_port: req.ssh_port,
            public_url: req.public_url.clone(),
            data_disks: req.data_disks.clone(),
        };
    }
    let typed = |text: &String| Some(text.trim().to_owned()).filter(|text| !text.is_empty());
    Values {
        target: req.target.as_ref().and_then(typed),
        ssh_port: Some(req.ssh_port.unwrap_or(SSH_PORT)),
        public_url: req.public_url.as_ref().and_then(typed).map(|url| {
            match url.strip_suffix('/') {
                // `https://host/` is the host: the command takes it so too.
                Some(host) if !host.ends_with('/') => host.to_owned(),
                _ => url,
            }
        }),
        data_disks: req.data_disks.iter().filter_map(typed).collect(),
    }
}

impl App {
    /// Installs the move helper's files (once; later calls are ignored and return `false`).
    pub fn set_move_helper(&self, helper: Arc<dyn MoveHelper>) -> bool {
        self.move_helper.set(helper).is_ok()
    }

    /// The facts of a move now; without a helper, of a host that has none.
    async fn migration_facts(&self) -> AppResult<Facts> {
        let now = self.now().0;
        let files = match self.move_helper.get().cloned() {
            // File reads and one bounded question to systemd.
            Some(helper) => tokio::task::spawn_blocking(move || read(helper.as_ref(), now))
                .await
                .map_err(AppError::internal)?,
            None => Files {
                installed: false,
                pending: None,
                helper: None,
                status: None,
                arrived: None,
            },
        };
        Ok(Facts {
            files,
            now,
            update_running: self.deps.store.open_update_run().await?.is_some(),
        })
    }

    /// Whether a move's request waits or its run is in flight: no update or restart starts then
    /// (SPEC-470 MVB-023). A helper that cannot be read holds nothing back.
    pub(crate) async fn move_in_flight(&self) -> bool {
        self.migration_facts()
            .await
            .is_ok_and(|facts| facts.waiting() == Some(true) || facts.running().is_some())
    }

    /// `GET /api/admin/instance/migration` (SPEC-470 MVB-020).
    ///
    /// # Errors
    /// `Forbidden` for non-superadmins; store failures.
    pub async fn migration_view(&self, ctx: &AuthContext) -> AppResult<MigrationView> {
        self.require_superadmin(ctx)?;
        Ok(self.migration_facts().await?.view(&self.config.public_url))
    }

    /// `POST /api/admin/instance/migration` (SPEC-470 MVB-021): writes the request for the root
    /// helper and answers the view.
    ///
    /// # Errors
    /// `Forbidden` for non-superadmins; `Invalid` for a field outside the rules or a move
    /// without the typed host; `Conflict` when nothing can be asked now.
    pub async fn ask_migration(
        &self,
        ctx: &AuthContext,
        req: &AskMigration,
    ) -> AppResult<MigrationView> {
        self.require_superadmin(ctx)?;
        let values = asked(req);
        let now = self.now();
        let request = MoveRequest {
            version: VERSION,
            id: format!("{}-{}", self.deps.crypto.new_id(), req.action.as_str()),
            action: req.action,
            target: values.target.clone(),
            ssh_port: values.ssh_port,
            public_url: values.public_url.clone(),
            data_disks: values.data_disks.clone(),
            requested_at: now.0,
        };
        rules::check_request(&request).map_err(AppError::Invalid)?;
        if req.action == MoveAction::Move {
            // Root on this host gives the instance's data to the host the request names: the
            // person types that host, here as on the page.
            let host = values.target.as_deref().map(rules::host_part);
            if req.confirm.as_deref() != host {
                return Err(AppError::Invalid(
                    "type the new host's name to confirm: root on this host will give this \
                     instance's data to it"
                        .into(),
                ));
            }
        }
        let Some(helper) = self.move_helper.get().cloned() else {
            return Err(AppError::Conflict(
                "a move cannot be asked from this server: it has no move helper".into(),
            ));
        };
        let facts = self.migration_facts().await?;
        if !facts.files.installed {
            return Err(AppError::Conflict(NOT_ENABLED.into()));
        }
        if facts.update_running {
            return Err(AppError::Conflict(
                "an update or restart is running: ask again once it has finished".into(),
            ));
        }
        if facts.waiting() == Some(true) {
            return Err(AppError::Conflict(
                "another request is waiting for the move helper".into(),
            ));
        }
        let running = facts.running();
        let in_flight = |what: &str| {
            AppError::Conflict(format!("{what} is running: wait for its end, or abort it"))
        };
        match (req.action, running) {
            (_, Some(MoveAction::Abort)) => return Err(in_flight("an abort")),
            (MoveAction::Plan | MoveAction::Move, Some(MoveAction::Plan)) => {
                return Err(in_flight("a plan"));
            }
            (MoveAction::Plan | MoveAction::Move, Some(MoveAction::Move)) => {
                return Err(in_flight("a move"));
            }
            (MoveAction::Move, None) => facts
                .movable(Some(&values))
                .map_err(|why| AppError::Conflict(why.into()))?,
            (MoveAction::Abort, _) if !facts.recorded() => {
                return Err(AppError::Conflict(
                    "no move is recorded on this host: nothing to abort".into(),
                ));
            }
            _ => {}
        }
        // What the abort is of, for the audit: the run it stops, or the recorded move.
        let target = values.target.clone().or_else(|| {
            let of_run = facts.run().and_then(|(run, _)| run.values.target.clone());
            of_run.or_else(|| facts.files.status.as_ref().map(|s| s.target.clone()))
        });
        let written = {
            let request = request.clone();
            tokio::task::spawn_blocking(move || helper.request(&request))
                .await
                .map_err(AppError::internal)?
        };
        written.map_err(|why| {
            AppError::Conflict(format!("the move's request could not be written: {why}"))
        })?;
        let login = self.login_of(&ctx.user.id).await;
        let mut detail = format!("request {}", request.id);
        if let Some(port) = values.ssh_port {
            detail.push_str(&format!("; port {port}"));
        }
        if let Some(url) = &values.public_url {
            detail.push_str(&format!("; public URL {url}"));
        }
        if !values.data_disks.is_empty() {
            detail.push_str(&format!("; data disks {}", values.data_disks.join(", ")));
        }
        let action = match req.action {
            MoveAction::Plan => "migration.planned",
            MoveAction::Move => "migration.started",
            MoveAction::Abort => "migration.aborted",
        };
        self.deps
            .store
            .audit(now, &login, action, target.as_deref(), Some(&detail))
            .await?;
        tracing::info!(
            actor = %login,
            action,
            target = target.as_deref().unwrap_or("the recorded move"),
            request = %request.id,
            "a move was asked from Settings"
        );
        Ok(self.migration_facts().await?.view(&self.config.public_url))
    }
}
