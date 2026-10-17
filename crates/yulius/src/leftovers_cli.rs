//! `yulius leftovers list|delete` (SPEC-215 LFT-012, ADR-0170): the leftovers on this server's
//! disk from SSH, with the server's own rules and live state. The command talks to the running
//! server's loopback listener with the key the server wrote to `<data>/leftovers-cli.key` (0600),
//! so only root and the service user can use it. `list` rescans first; `delete` rescans, shows
//! what it will delete with the total size and asks for `yes` (unless `--yes`), then follows the
//! server's background deletion to its end.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use yulius_app::leftovers::CLI_KEY_FILE;
use yulius_contracts::api::leftovers::{DeleteLeftoversRequest, LeftoverView, LeftoversView};
use yulius_domain::disk::human;

/// The server's loopback listener as `yulius serve` binds it by default (`--url` for another
/// `YULIUS_INTERNAL_BIND`).
pub const DEFAULT_URL: &str = "http://127.0.0.1:8081";
/// How long a scan or a deletion is followed at most.
const JOB_LIMIT: Duration = Duration::from_secs(60 * 60);
/// How often a running scan or deletion is read.
const POLL: Duration = Duration::from_millis(500);
/// One request's limit.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// What to do.
#[derive(Debug, Clone)]
pub enum Action {
    /// Rescan and print every leftover.
    List {
        /// Print the server's JSON instead of the table.
        json: bool,
    },
    /// Rescan, then delete every safe item or the given ids.
    Delete {
        /// Every item Yulius deletes by itself.
        all_safe: bool,
        /// Ids from `list`.
        ids: Vec<String>,
        /// Skip the typed confirmation.
        yes: bool,
    },
}

struct Server {
    client: reqwest::Client,
    url: String,
    key: String,
}

impl Server {
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<LeftoversView, String> {
        let response = request
            .bearer_auth(&self.key)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("cannot reach the server at {}: {e}", self.url))?;
        let status = response.status();
        let body = response.text().await.map_err(|e| e.to_string())?;
        if status.is_success() {
            return serde_json::from_str(&body).map_err(|e| format!("a bad answer: {e}"));
        }
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
            .unwrap_or(body);
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(format!(
                "the server refused the key in {CLI_KEY_FILE} (it writes a new one at every \
                 start; is this its data directory?): {message}"
            ));
        }
        Err(format!("{status}: {message}"))
    }

    async fn view(&self) -> Result<LeftoversView, String> {
        self.send(self.client.get(format!("{}/internal/leftovers", self.url)))
            .await
    }

    async fn post(
        &self,
        path: &str,
        body: &DeleteLeftoversRequest,
    ) -> Result<LeftoversView, String> {
        self.send(
            self.client
                .post(format!("{}/internal/leftovers{path}", self.url))
                .json(body),
        )
        .await
    }

    /// Waits until no scan or deletion runs; returns the view then.
    async fn settled(&self) -> Result<LeftoversView, String> {
        let deadline = Instant::now() + JOB_LIMIT;
        loop {
            let view = self.view().await?;
            if !view.job.as_ref().is_some_and(|job| job.running) {
                return Ok(view);
            }
            if Instant::now() >= deadline {
                return Err("the server's scan or deletion is still running".into());
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// Rescans (after any scan or deletion that runs) and returns the fresh list.
    async fn rescan(&self) -> Result<LeftoversView, String> {
        self.settled().await?;
        self.post("/scan", &DeleteLeftoversRequest::default())
            .await?;
        self.settled().await
    }
}

fn size(item: &LeftoverView) -> String {
    item.bytes.map_or_else(|| "unknown".to_owned(), human)
}

fn label(item: &LeftoverView) -> &'static str {
    if !item.deletable {
        "report only"
    } else if item.auto {
        "safe"
    } else {
        "needs you"
    }
}

fn print_items(out: &mut dyn Write, items: &[LeftoverView]) {
    for item in items {
        let owner = match (&item.sandbox, item.task) {
            (Some(sandbox), Some(task)) => format!("{sandbox} #{task}"),
            (Some(sandbox), None) => sandbox.clone(),
            _ => "instance".to_owned(),
        };
        let _ = writeln!(
            out,
            "{}  {:>9}  {:<11}  {:<18}  {}  ({owner})",
            item.id,
            size(item),
            label(item),
            item.kind,
            item.path
        );
        let _ = writeln!(out, "    {}", item.reason);
    }
}

fn print_disk(out: &mut dyn Write, view: &LeftoversView) {
    if let (Some(used), Some(total)) = (view.disk_used, view.disk_total)
        && total > 0
    {
        let _ = writeln!(
            out,
            "Disk of {}: {} of {} used ({}%).",
            view.data_dir,
            human(used),
            human(total),
            used.saturating_mul(100) / total
        );
    }
    let _ = writeln!(
        out,
        "Reclaimable: {} ({} safe to delete automatically).",
        human(view.total_bytes),
        human(view.safe_bytes)
    );
}

fn read_key(data_dir: &Path) -> Result<String, String> {
    let path = data_dir.join(CLI_KEY_FILE);
    let key = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "cannot read {} ({e}): run as root or the service user, with the server's data \
             directory (--data-dir), while the server runs",
            path.display()
        )
    })?;
    let key = key.trim().to_owned();
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{} holds no key", path.display()));
    }
    Ok(key)
}

/// Runs `action` against the server at `url` whose data directory is `data_dir`; `confirm`
/// answers the typed confirmation. Exit code: 0 done, 1 a deletion failed, 2 not run.
pub async fn run_async(
    out: &mut dyn Write,
    data_dir: &Path,
    url: &str,
    action: Action,
    confirm: &mut dyn FnMut(&str) -> bool,
) -> i32 {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let key = match read_key(data_dir) {
        Ok(key) => key,
        Err(err) => {
            let _ = writeln!(out, "yulius leftovers: {err}");
            return 2;
        }
    };
    let server = Server {
        client: reqwest::Client::new(),
        url: url.trim_end_matches('/').to_owned(),
        key,
    };
    match go(out, &server, action, confirm).await {
        Ok(code) => code,
        Err(err) => {
            let _ = writeln!(out, "yulius leftovers: {err}");
            2
        }
    }
}

async fn go(
    out: &mut dyn Write,
    server: &Server,
    action: Action,
    confirm: &mut dyn FnMut(&str) -> bool,
) -> Result<i32, String> {
    let _ = writeln!(out, "Scanning the disk…");
    let _ = out.flush();
    let view = server.rescan().await?;
    match action {
        Action::List { json } => {
            if json {
                let text = serde_json::to_string_pretty(&view).map_err(|e| e.to_string())?;
                let _ = writeln!(out, "{text}");
            } else {
                print_items(out, &view.items);
                if view.items.is_empty() {
                    let _ = writeln!(out, "Nothing left over.");
                }
                print_disk(out, &view);
            }
            Ok(0)
        }
        Action::Delete { all_safe, ids, yes } => {
            if all_safe == !ids.is_empty() {
                return Err(
                    "give --all-safe or the ids to delete (from `yulius leftovers list`)".into(),
                );
            }
            let chosen: Vec<LeftoverView> = view
                .items
                .iter()
                .filter(|i| {
                    if all_safe {
                        i.auto
                    } else {
                        ids.contains(&i.id)
                    }
                })
                .cloned()
                .collect();
            let unknown: Vec<&String> = ids
                .iter()
                .filter(|id| !chosen.iter().any(|i| &i.id == *id))
                .collect();
            if !unknown.is_empty() {
                return Err(format!(
                    "no such leftover (any more): {}",
                    unknown
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if chosen.is_empty() {
                let _ = writeln!(out, "Nothing to delete.");
                return Ok(0);
            }
            let total: u64 = chosen.iter().filter_map(|i| i.bytes).sum();
            print_items(out, &chosen);
            let question = format!(
                "Delete {} leftover(s), {}? Deleted files cannot be restored. Type yes: ",
                chosen.len(),
                human(total)
            );
            if !yes && !confirm(&question) {
                let _ = writeln!(out, "Nothing deleted.");
                return Ok(2);
            }
            let request = DeleteLeftoversRequest {
                ids: if all_safe {
                    Vec::new()
                } else {
                    chosen.iter().map(|i| i.id.clone()).collect()
                },
                all_safe,
            };
            server.post("/delete", &request).await?;
            let done = server.settled().await?;
            let job = done.job.clone().unwrap_or_default();
            let _ = writeln!(
                out,
                "Deleted {} item(s), {} freed.",
                job.deleted,
                human(job.freed)
            );
            for kept in &job.kept {
                let _ = writeln!(out, "Kept: {kept}");
            }
            for failed in &job.failed {
                let _ = writeln!(out, "Failed: {failed}");
            }
            print_disk(out, &done);
            Ok(i32::from(!job.failed.is_empty()))
        }
    }
}

/// `yulius leftovers …` from `main`: confirmations are read from the terminal.
pub fn run(data_dir: &Path, url: &str, action: Action) -> i32 {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("yulius leftovers: {err}");
            return 2;
        }
    };
    let mut confirm = |question: &str| {
        print!("{question}");
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer).is_ok() && answer.trim() == "yes"
    };
    let data_dir: PathBuf = data_dir.to_path_buf();
    runtime.block_on(run_async(
        &mut std::io::stdout(),
        &data_dir,
        url,
        action,
        &mut confirm,
    ))
}
