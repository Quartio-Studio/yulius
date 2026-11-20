//! The personal Telegram channel's intake (SPEC-512 part 4, #1107, ADR-0392): with intake on, a
//! linked chat's reply to the bot's message about a task is the person's `task_message` to it,
//! under the owner rule (TGI-002); any other message, its files too, starts a task in their
//! default project (TGI-003); `/status` and `/task` (TGI-004). Off, nothing happens (TGI-001).
//! Everything runs as the linked person, with their access, and the answers that carry a task's
//! text are redacted as transcripts are (TGI-005).

use std::sync::Arc;

use serde_json::json;
use yulius_contracts::api::tasks::CreateTask;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::notify::IntakeCommand;
use yulius_domain::permission::Action;
use yulius_domain::task::{Activity, TaskStatus};

use crate::auth::AuthContext;
use crate::context::App;
use crate::coordination::{Channel, ProjectCaller};
use crate::error::{AppError, AppResult};
use crate::model::{TaskOrigin, UserRecord};
use crate::ports::notify::{ChatLink, Inbound, InboundFile};
use crate::telegram::ChannelServices;

/// Telegram's bot download bound: a file over it is refused (TGI-003).
pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
/// The most files one message starts a task with.
pub const MAX_FILES: usize = 10;
/// The most tasks `/status` lists.
const STATUS_LISTED: usize = 20;
/// The answer to `/help` and to a command the chat does not know, with intake on.
pub const INTAKE_HELP: &str = "This chat gets your Yulius notifications and takes work:\n\
     • reply to a notification to answer its task;\n\
     • send a message, a photo or a file to start a task in your default project;\n\
     • /task <project> <text> starts one in another project;\n\
     • /status lists what waits for you;\n\
     • /stop unlinks this chat.";

impl App {
    /// A linked chat's message with intake on (TGI-002..TGI-004): what the bot answers.
    pub(crate) async fn telegram_intake(
        self: &Arc<Self>,
        link: &ChatLink,
        message: &Inbound,
    ) -> AppResult<String> {
        let Some(person) = self
            .deps
            .store
            .user(&link.user)
            .await?
            .filter(|u| u.disabled_at.is_none())
        else {
            return Ok("This Yulius account is disabled.".into());
        };
        if let Some(reply_to) = &message.reply_to
            && let Some(task) = self.deps.store.sent_about(&message.chat, reply_to).await?
        {
            return self.telegram_reply(&person, &task, message).await;
        }
        match yulius_domain::notify::intake_command(&message.text) {
            IntakeCommand::Status => self.telegram_status(&person).await,
            IntakeCommand::Help => Ok(INTAKE_HELP.into()),
            IntakeCommand::TaskWithoutProject => {
                Ok("Write /task <project> <text>, with the project's name.".into())
            }
            IntakeCommand::Task { project, text } => {
                let Some(sandbox) = self.project_named(&person, project).await? else {
                    return Ok(format!(
                        "No project you may work in is called {project}. Write /task <project> \
                         <text>."
                    ));
                };
                self.telegram_task(&person, &sandbox, text, &message.files)
                    .await
            }
            IntakeCommand::Text => {
                let Some(sandbox) = link
                    .intake_sandbox
                    .as_deref()
                    .and_then(|id| SandboxId::parse(id).ok())
                else {
                    return Ok("Choose the project new messages start tasks in: Yulius, \
                               Settings → Notifications. Or write /task <project> <text>."
                        .into());
                };
                self.telegram_task(&person, &sandbox, message.text.trim(), &message.files)
                    .await
            }
        }
    }

    /// TGI-002: the reply is the person's `task_message` to `task`, with the guards of their
    /// own agent's (the owner rule, a named person's answer, the owner's personal reach).
    async fn telegram_reply(
        self: &Arc<Self>,
        person: &UserRecord,
        task: &TaskId,
        message: &Inbound,
    ) -> AppResult<String> {
        let Some(record) = self.deps.store.task(task).await? else {
            return Ok("That task is gone.".into());
        };
        let text = message.text.trim();
        if text.is_empty() {
            return Ok(format!(
                "A reply to #{} reaches it as text: write what to tell it.",
                record.number
            ));
        }
        let caller = ProjectCaller {
            sandbox: record.sandbox_id.clone(),
            user: person.id.clone(),
            task: None,
            channel: Channel::Remote,
            actor: format!("telegram:{}", person.label()),
            detail: json!({ "via": "telegram" }),
        };
        let mut answer = match self
            .tool_task_message(&caller, &json!({ "task_id": task.as_str(), "text": text }))
            .await
        {
            Ok(_) => format!("Sent to #{}.", record.number),
            Err(err) => format!("Not sent to #{}: {}", record.number, err.public_message()),
        };
        if !message.files.is_empty() {
            answer.push_str(" Files in a reply are not sent: attach them on the task's page.");
        }
        Ok(self.redact_for_sandbox(&record.sandbox_id, &answer).await)
    }

    /// TGI-004: the person's tasks that wait for them, as the digest's first section.
    async fn telegram_status(&self, person: &UserRecord) -> AppResult<String> {
        let store = &self.deps.store;
        let login = person.github_login.as_deref().map(str::to_lowercase);
        let mut lines = Vec::new();
        let mut more = 0_usize;
        for task in store
            .tasks_with_status(&[TaskStatus::Running, TaskStatus::Parked])
            .await?
        {
            if task.archived_at.is_some() {
                continue;
            }
            let asked = store.task_needs_you(&task.id).await?;
            let names_them = asked.as_ref().is_some_and(|a| {
                login
                    .as_ref()
                    .is_some_and(|login| a.named.who.iter().any(|w| w.to_lowercase() == *login))
            });
            let waits = asked.is_some() || task.activity == Some(Activity::NeedsAttention);
            if !waits || !(task.owner == person.id || names_them) {
                continue;
            }
            if !self.can_view(person, &task.sandbox_id).await {
                continue;
            }
            if lines.len() >= STATUS_LISTED {
                more += 1;
                continue;
            }
            let Some(sandbox) = store.sandbox(&task.sandbox_id).await? else {
                continue;
            };
            let what = asked.map_or_else(|| "a question in its terminal".to_owned(), |a| a.action);
            let line = format!(
                "• {} #{} {}: {what}\n  {}/s/{}/t/{}",
                sandbox.repo_name,
                task.number,
                task.title,
                self.config.public_url.trim_end_matches('/'),
                sandbox.slug,
                task.number
            );
            lines.push(self.redact_for_sandbox(&task.sandbox_id, &line).await);
        }
        if lines.is_empty() {
            return Ok("Nothing waits for you.".into());
        }
        let mut out = format!(
            "Waiting for you ({}):\n{}",
            lines.len() + more,
            lines.join("\n")
        );
        if more > 0 {
            out.push_str(&format!("\n…and {more} more in Yulius."));
        }
        Ok(yulius_domain::notify::truncate(&out))
    }

    /// The project `name` (its repository name or slug) the person may work in.
    async fn project_named(&self, person: &UserRecord, name: &str) -> AppResult<Option<SandboxId>> {
        let name = name.to_lowercase();
        for sandbox in self.deps.store.sandboxes().await? {
            if sandbox.removed_at.is_some() {
                continue;
            }
            let short = sandbox
                .repo_name
                .rsplit('/')
                .next()
                .unwrap_or(&sandbox.repo_name)
                .to_lowercase();
            if (sandbox.slug.to_lowercase() == name
                || sandbox.repo_name.to_lowercase() == name
                || short == name)
                && self
                    .require(person, &sandbox.id, Action::Work)
                    .await
                    .is_ok()
            {
                return Ok(Some(sandbox.id));
            }
        }
        Ok(None)
    }

    /// TGI-003: a task in `sandbox` as the person, its files as attachments named in its prompt,
    /// with `task_create`'s bound, onboarding gate and access checks; labelled `telegram`.
    async fn telegram_task(
        self: &Arc<Self>,
        person: &UserRecord,
        sandbox: &SandboxId,
        text: &str,
        files: &[InboundFile],
    ) -> AppResult<String> {
        if text.is_empty() && files.is_empty() {
            return Ok("Send text or a file to start a task.".into());
        }
        if files.len() > MAX_FILES {
            return Ok(format!(
                "At most {MAX_FILES} files start a task; this message has {}.",
                files.len()
            ));
        }
        if let Err(err) = self.require(person, sandbox, Action::Work).await {
            return Ok(format!("Not started: {}", err.public_message()));
        }
        let ctx = AuthContext {
            user: person.clone(),
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let mut paths = Vec::new();
        for file in files {
            match self.telegram_attachment(&ctx, sandbox, file).await {
                Ok(path) => paths.push(path),
                Err(reason) => return Ok(format!("Not started: {} {reason}", file.name)),
            }
        }
        let mut prompt = text.to_owned();
        if !paths.is_empty() {
            if !prompt.is_empty() {
                prompt.push_str("\n\n");
            }
            prompt.push_str("Attached from Telegram:");
            for path in &paths {
                prompt.push_str(&format!("\n- {path}"));
            }
        }
        let req = CreateTask {
            prompt,
            harness: String::new(),
            workflow_id: None,
            model: None,
            effort: None,
            title: None,
            mode: None,
            after_task_id: None,
            start_rule: None,
            supertask: false,
            prepared: false,
            start_at: None,
        };
        match self
            .create_task_as(person, sandbox, &req, TaskOrigin::Telegram)
            .await
        {
            Ok(task) => {
                let slug = self
                    .deps
                    .store
                    .sandbox(sandbox)
                    .await?
                    .map_or_else(String::new, |s| s.slug);
                Ok(format!(
                    "Started #{} from Telegram: {}/s/{slug}/t/{}",
                    task.number,
                    self.config.public_url.trim_end_matches('/'),
                    task.number
                ))
            }
            Err(err) => Ok(format!("Not started: {}", err.public_message())),
        }
    }

    /// Fetches one received file (at most [`MAX_FILE_BYTES`]) into the person's drafts folder;
    /// its path, or why not.
    async fn telegram_attachment(
        &self,
        ctx: &AuthContext,
        sandbox: &SandboxId,
        file: &InboundFile,
    ) -> Result<String, String> {
        if file.size.is_some_and(|size| size > MAX_FILE_BYTES) {
            return Err("is over Telegram's 20 MB bot download bound.".into());
        }
        let bot = self
            .channel
            .get()
            .and_then(ChannelServices::active)
            .ok_or_else(|| "could not be fetched: the bot is not running.".to_owned())?;
        let bytes = bot
            .channel
            .fetch_file(&file.id, MAX_FILE_BYTES)
            .await
            .map_err(|err| match err {
                crate::ports::notify::ChannelError::TooLarge => {
                    "is over Telegram's 20 MB bot download bound.".to_owned()
                }
                _ => "could not be fetched from Telegram.".to_owned(),
            })?;
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let saved: AppResult<String> = async {
            let mut upload = self
                .begin_draft_attachment(ctx, sandbox.as_str(), &file.name, Some(len))
                .await?;
            upload.write(&bytes).await?;
            Ok(self.finish_upload(upload).await?.path)
        }
        .await;
        saved.map_err(|err: AppError| format!("could not be saved: {}", err.public_message()))
    }
}
