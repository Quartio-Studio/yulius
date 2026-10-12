# Product summary

Yulius is a self-hosted coding workspace for the Quartio team, written in Rust and inspired by
Cezar and Open Mercato Cloud. It runs native Claude Code sessions, one per task, inside
contained sandboxes on a single Linux VM, and enforces engineering discipline around them.
Yulius develops itself with Yulius.

The authoritative product brief is [SPEC-001](../.ai/specs/SPEC-001-yulius-product.md); this
page is the short version for anyone who must know the whole product before building.

## Who it is for

A small team (2-5 people; the founders are superadmins) that works with several Claude
subscriptions and many repositories and wants agents to deliver autonomously, with people
watching, steering and unblocking.

## What it does

- **Projects and sandboxes**: import a GitHub repository (or onboard it from a local machine's
  Claude sessions with `yulius onboard`); each gets a contained sandbox with its own system
  layer, home, shells per user, browser and editor.
- **Tasks**: each task runs a real Claude Code CLI in a PTY on its own git worktree. Sessions
  outlive server restarts. Task intake splits mixed requests, holds scope changes and chains
  dependent tasks, also by a logical rule such as "#12 done and #13 done", which the composer
  builds from picked conditions (a task, its state, all or any).
- **Subscription pool**: Claude accounts are used as one self-managing pool; tasks are
  subscription-agnostic, switches and pauses are invisible to agents.
- **Delivery discipline**: spec when it matters, effort-weighted steps, independent review,
  merge on green into `dev`, promotion to `main`, deploy and verify, cleanup. Profiles live in
  `yulius.toml`.
- **Yulius CI**: the projects' only CI; it runs on the VM, no GitHub Actions.
- **Browser panel**: the sandbox's real Chrome streamed as video so people can pass captchas
  and sign-ins for the agent.
- **Editor, terminals, Git view, messages, automations, workflow catalog** in one web UI.
- **Agent primitives as code**: Yulius's own skills are APM packages with a lockfile, deployed to
  every harness and checked for drift; onboarding codifies a project's conventions as primitives.
- **Secrets vault**: team secrets people reveal; agent secrets agents use but never see.
- **Updates**: Yulius updates itself from `main` without losing running work.

## Principles

Agents are real CLIs; sessions outlive the server; GitHub is the authority for permissions;
coordination goes through one MCP and hook protocol; every queue, stream, retry and wait is
bounded and owned; features have specs, research and questions do not.

## Where to read more

- Architecture: [architecture.md](architecture.md); decisions: [decisions](decisions/README.md).
- Requirements and evidence: [SPEC-001](../.ai/specs/SPEC-001-yulius-product.md),
  [SPEC-002](../.ai/specs/SPEC-002-yulius-architecture.md),
  [SPEC-003](../.ai/specs/SPEC-003-traceability.md).
- Operations: [operations.md](operations.md); API: [api.md](api.md).
- Change history: [changes.md](changes.md).
