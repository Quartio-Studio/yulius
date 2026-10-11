# SPEC-003: Source traceability and self-review

Status: accepted
Date: 2026-10-07
Owner: implementing agent
Scope authorization: user goal of 2026-10-07 (trace requirements to timestamps and video
evidence; review the specification against the originals).

This review maps every segment range of the Oct 6 transcript (all 295 segments, indices
`[000]`–`[294]`), every morning note and every draft-prompt item to requirements in
[SPEC-001](SPEC-001-yulius-product.md) or to an explicit classification. A row count is not proof of
coverage; the self-review in §5 records what was re-checked and what changed.

## 1. Transcript coverage (in order)

| Segments | Time | Content (paraphrase) | Maps to |
| --- | --- | --- | --- |
| 000–008 | 00:00–02:28 | Mic test, recording, start screen share | meta (no requirement) |
| 009–013 | 02:31–03:33 | Liked Cezar's concept, it has problems; build a separate product for weaponized vibe-coding that imposes discipline; Open Mercato workflows and interface stay | §2 Vision, UI-001, AUT-004 |
| 014–016 | 03:38–03:56 | Go through the points written this morning | WA list (§2 below) |
| 017–023 | 04:00–04:47 | Layout: panel (left), space (center), browser+editor (right); not on by default, icons that expand; everything except left is the terminal | UI-001, UI-002, UI-003 (V 4:04–4:24 pointing) |
| 024–027 | 04:47–04:55 | Like Claude in a terminal: bar on the left, middle is … | UI-003, HRN-001 |
| 028–038 | 04:55–05:19 | Tasks like Cezar's; create a task, tasks appear at the bottom; click Active | UI-004, UI-005 (V 5:04–5:36) |
| 039–041 | 05:19–05:28 | This part stays: Workflow, Skills, Automation | UI-004, AUT-004, SKL-005 |
| 042–043 | 05:31–05:33 | Settings not needed for MVP | ACC-010 (C over P(2)) |
| 044–049 | 05:36–05:50 | Active stays; Archive moved elsewhere | UI-006 |
| 050–055 | 05:50–05:59 | Web hidden by default, editor too; "what is the editor for… but OK, keep it" | UI-002, EDT-001 |
| 056 | 06:00–06:21 | Top usage bar removed; CPU/GPU/RAM into a hidden status/usage panel | UI-007, Status panel (V 6:00) |
| 057–064 | 06:21–06:50 | Project name header pointless; second-level thin tabs (≤ "Coding Agent / Terminal Logs" row) listing "Quartio Studio", "Sandbox 1", "Sandbox 2" instead of Terminal/Logs | UI-008, PRJ-006 (V 6:33–6:44) |
| 065–068 | 06:50–07:05 | Web is broken in the reference; should be a normal Chrome-like browser, any address; global for the whole VM | BRW-001; "global" superseded |
| 069–072 | 07:07–07:56 | Agents use it too; global or per user is messy (AWS login) → per sandbox; nice to have tab management; jump to the tab where an action is requested | BRW-002 (C), BRW-003, BRW-004 |
| 073–077 | 07:56–08:12 | Agent writes "click, open this tab" in chat; links in Claude are hard to navigate | BRW-004, UI-011 |
| 078–082 | 08:13–08:37 | Right part done; editor = plain VS Code | EDT-001 |
| 083–086 | 08:37–09:39 | Permissions copied from GitHub; org GitHub or private account; each project a GitHub repo (Product/Studio imports, new projects); inherited permissions; one sandbox = one repo; many sandboxes per instance | ACC-006, ACC-007, PRJ-001, PRJ-003 |
| 087–102 | 09:39–10:28 | One VM, many isolated sandboxes; wish to check another repo from a sandbox dropped — MCP covers it; coding rules will live in Yulius | PRJ-001, PRJ-002, PRJ-007, DSC-001/005 |
| 103–109 | 10:31–11:05 | Accounts topic; center up to the bar must be exactly the native Claude/Codex session interface — a terminal | HRN-001 (V 10:38–10:59 selection) |
| 110–113 | 11:06–11:22 | Cannot exit the terminal; leaving the session closes the task; a separate terminal exists (V 11:12 reference Terminal tab), this one is dedicated to Claude/Codex | HRN-002, UI-015 (U-11) |
| 114–117 | 11:25–11:41 | One task = one session; harness chosen at creation; Claude and Codex only for now | TSK-001..003 |
| 118–126 | 11:41–13:01 | Many accounts per provider; switch at exhaustion or ~99%; invisible; use resets first; then switch provider, visible in the terminal yet smooth; elegant | ROT-001, ROT-003, ROT-004 (U-03), ROT-005 |
| 127 | 13:04–13:27 | Harness features (switching sessions) conflict with one session = one task; handle smartly | HRN-004 |
| 128–131 | 13:27–13:50 | Tasks see previous and parallel tasks; Yulius-level communication protocol independent of Claude/Codex; "additionally a protocol which…" (deferred to discipline, unfinished) | COM-001, COM-002 |
| 132–135 | 13:56–14:45 | Strong account orchestrator (10 Codex, 15 Claude); effort changes; native `/model`, `/effort`, `/fast`; session translation across providers is non-trivial and needs testing | ROT-001, ROT-008, HRN-003, ROT-005, ROT-006 |
| 136–147 | 14:45–15:09 | Header panel annoys; finish UI first; big things to remove; layout breaks | UI-009 (V 14:58 broken header) |
| 148–155 | 15:11–16:00 | Likes the step bar "maybe not in this form"; Yulius' own step/todo manager, auditable, where the agent is and what remains; commits/files tabs removed (files = editor); very simplified | STP-001..003, UI-009, U-04 |
| 156–158 | 16:04–16:23 | "claude default" belongs at the bottom where commands are typed; "done" status → icons on the left | UI-010, UI-005 |
| 159–164 | 16:23–16:59 | Discipline; product assumption: push vibe-coding to the limit; definition of vibe-coding | §2 Vision, DSC-012 |
| 165–167 | 17:00–17:35 | Two ways: MCP delegated to Yulius, attached to both harnesses; Quartio MCP feeds product knowledge | MCP-001, MCP-002 (X) |
| 168–170 | 17:35–18:59 | Enforce docs format; one task = one spec/implementation plan; Yulius enforces todos/sections and prompts the agent; architecture and other diagrams; docs and decisions as Markdown next to code | DSC-001..005 |
| 171 | 19:00–19:12 | New project gets the format automatically; import is harder | PRJ-005, DSC-010 |
| 172–173 | 19:13–19:59 | Non-programmer wants production-level code; models insufficient; bugs | DSC-012, §2 Vision |
| 174–177 | 20:00–20:40 | New/converted repo: technology decisions documented and made by the agent (Python for data, C++/Rust for speed — examples) | DSC-006 (X for languages) |
| 178–180 | 20:40–21:10 | Implementers may research documentation practice; for now one task = one spec | DSC-011 |
| 181–186 | 21:10–21:59 | Git discipline: no direct pushes to production, PRs; see Cezar → rather this Yulius repo and our other repos; one format now, templates later | DSC-007, DLV-002, DSC-001 (D templates) |
| 187–192 | 22:00–23:22 | Limit bugs/slop with maximum simplicity; online store and 3D model examples; new task = one feature → spec → branch → per-spec implementation → tests, QA → dev → main; testing at every stage; deployment within it | DSC-008, DLV-001/002 (X for store/3D) |
| 193–197 | 23:22–23:53 | Browser done; sessions communicate and agree scopes; each fix its own worktree; communication independent of harness | COM-002, COM-003, TSK-005 |
| 198–205 | 23:53–24:37 | Skills: no slash commands for non-technical users; automatic activation (design example); take skills from our existing repository | SKL-001, SKL-002 (X design) |
| 206–207 | 24:37–24:47 | Workflows + skills tabs; manage Git and automations | UI-004, GIT-001, AUT-004 |
| 208–212 | 24:48–25:44 | Other product collects customer knowledge/feedback; daily task implementing last day's feedback; create automations; ask the agent to create an automation/workflow | AUT-001 (X daily feedback), AUT-002 |
| 213–214 | 25:45–26:59 | Every project needs a GitHub repo, enforced; `yulius import` CLI with credentials, rsync-like sync; import Claude/Codex sessions from a prompted period (e.g., two weeks), only those of the repository | IMP-001..003 |
| 215 | 27:00–27:44 | Imported project may not comply; preparation meta-task first; minimal attention (approve or informed); tidy docs | DSC-010, IMP-005, U-08 |
| 216–217 | 27:44–27:50 | Anything else for import? No | — |
| 218–221 | 27:50–28:33 | Imported sessions of different people are organization sessions; anyone can continue another's session, even switching to Codex; maximally simple | IMP-004, TSK-007 |
| 222–224 | 28:38–29:18 | Sandbox keeps running after closing the window; control from phone; many sandboxes per VM installation, each project a repo | TSK-004, UI-012, PRJ-001 |
| 225–237 | 29:19–30:21 | Local computer? For now VM; server usable from a desktop app (Electron); local server later; migrate instance to VM later; deploy our space on julius.quartio.ai | OPS-001 (U-01), OPS-002, OPS-003 (D) |
| 238–241 | 30:22–30:51 | Superadmin at install; password not recoverable for now (needs email); strong UI warning to save it; superadmin creates users | ACC-001, ACC-002 (C over P(3)), ACC-004 |
| 241–247 | 30:52–31:43 | Every user GitHub-authenticated; org or private GitHub; inherited permissions (read-only → read tasks; no access → invisible); consistent | ACC-005..008 |
| 248–249 | 31:44–32:05 | Easy login to Claude, Codex, GitHub via OAuth, button | ROT-002, ACC-005 |
| 250–251 | 32:06–32:27 | Create or import; basic templates; auto-create GitHub repo; synchronized | PRJ-003, PRJ-004, PRJ-008 |
| 252–254 | 32:28–32:59 | Automatic skills covered; MCP independent of harness: Quartio MCP, Apify MCP (scrape X), anything | MCP-001, MCP-002 (X) |
| 255–259 | 33:00–33:28 | Open Mercato skills: include the useful ones for code discipline, not all; comb and decide | SKL-003, §7.3 |
| 260–261 | 33:29–33:43 | Clickable links; subscription management covered; agent creates automations/cron jobs | UI-011, ROT-009, AUT-002, AUT-003 |
| 262–268 | 33:44–34:05 | "Archive chats works great": archiving is great but the button need not be permanent; "my archived chats" with restore | UI-006, AC-ARC-1 |
| 269–274 | 34:06–35:05 | Discipline again: decisions inconsistent across sessions without a project summary; consistent docs, good specs, summaries of recent changes; one task with full project context — most important | COM-004, DSC-009, §2 success measure |
| 275–279 | 35:06–35:44 | Background analytics: duration, model, user feedback (correct or not), tokens; full analytics, very important | ANL-001..003 |
| 280–281 | 35:44–35:52 | That is all from WhatsApp; now the prompt written so far | P(n) list (§3 below) |
| 282–285 | 35:53–36:37 | Like Cezar, need different foundations; that is all; assume and decide the rest; harness for solid vibe-coding for 2–5 people or one, consistent, no slop; everything documented | §2 Vision (team size supersedes P(1) "2–3") |
| 286–290 | 36:38–37:10 | Which model to run the spec with | meta (not a product requirement) |
| 291–294 | 37:29–37:33 | Send the spec; end | meta |

## 2. Morning notes (WA) coverage

| Note | Maps to |
| --- | --- |
| 13:02 Browser should work (for Yulius) | BRW-001 |
| 13:04 Per-user browser, controllable 1:1 like native Claude/Codex plugins | BRW-003; per-user superseded by T 7:26 per sandbox (BRW-002) |
| 13:05 Sessions communicate regardless of Codex/Claude | COM-002 |
| 13:07 Skill orchestration | SKL-004 |
| 13:08 workflows, skills, automations, git, github | UI-004, GIT-001, AUT-*, SKL-* |
| 13:09 Easy migration of projects and sessions (choose session range) | IMP-001..003 |
| 13:09 editor | EDT-001 |
| 13:09 switching between projects | PRJ-006, UI-008 |
| 13:10 easy login to claude, codex, github | ROT-002, ACC-005 |
| 13:10 templates | PRJ-004, PRJ-008 |
| 13:11 Automatic skills | SKL-001 |
| 13:11 mcps | MCP-001 |
| 13:11 From OM | SKL-003 |
| 13:12 clickable links | UI-011 |
| 13:12 subscription management | ROT-009 |
| 13:13 Automations and cron jobs; settable by prompts | AUT-001..003 |
| 13:17 unsure about OM skills | resolved at T 33:00–33:22 (SKL-003) |
| 13:27 archive chats works great | UI-006 |
| 13:34 phone support | UI-012, J5 |
| 13:35 enforcing specs | DSC-001..003 |
| 13:36 whatsapp | U-07 (no requirement) |
| 13:38 onboarding after import: "do you want to reverse-engineer?" | DSC-010 |
| 13:39 consistent model of basic git hygiene | DSC-007 |
| 13:40 mandatory GitHub connection for every user | ACC-005 |
| 13:44 project summary, what happened recently | DSC-009, COM-004 |

## 3. Draft prompt (P) coverage — predates the call

| Item | Maps to | Precedence note |
| --- | --- | --- |
| P(1) vision; documentation discipline; teams of 2–3; keeps running when the website is closed; analyze both repos; tasks by Claude/Codex, documented, talking, easy harness/account switch | §2, DSC-*, TSK-004, COM-002, ROT-007, R:product/R:studio review | Team size 2–5 (T 36:07) supersedes 2–3 |
| P(2) dashboard, tasks, git, automations, skills, workflows, settings; Open Mercato sandbox view (harness left, editor + browser right) | UI-001, UI-004 | Settings dropped (T 5:31); browser/editor hidden by default (T 4:20) |
| P(3) open source; org VM; yulius.quartio.ai; one superadmin; password set at start; recovery code; not recoverable; strong UI warning | OPS-001, ACC-001/002, U-01, U-02 | "Recovery code" superseded by T 30:22 (not recoverable for now) |
| P(4) one instance = one org or private profile; every user connects GitHub; project = one repo, linked on onboarding or created; same permissions as GitHub; proxy for GitHub | ACC-005..007, PRJ-001/003/004 | consistent with call |
| P(5) orchestrator for existing agents; many accounts; one session = one task; same TUI ("seriously TUI", image of a native TUI); rotation and provider translation; manual switching; harness chosen per task | TSK-*, HRN-*, ROT-* | consistent with call |
| P(6) discipline is the reason the product exists | §2, DSC-* | — |
| P(7) mcps; "Example usecase:" unfinished | MCP-001 | unfinished text ignored |

## 4. Video observations used

| Time | Observation | Used for |
| --- | --- | --- |
| V 2:30–4:00 | Open Mercato sandbox page with Cezar inside; top usage bar; "AI Coding Agent / Terminal / Logs" row; Web/Editor panel previewing the app | Layout reference (UI-001..008), PRJ-010 |
| V 4:04–4:24 | Cursor on left nav, center, Web/Editor tabs while describing zones | UI-001 |
| V 4:56–5:36 | Skills page, Tasks list, new-task dialog (worktree, autonomous, plan first), session with step bar; Active/Archived at sidebar bottom | UI-004, UI-005, U-09 |
| V 5:54 | Editor tab is a browser VS Code | EDT-001 |
| V 6:00–6:44 | Cursor on top usage bar, project title, the Terminal/Logs row | UI-007, UI-008 |
| V 6:54 | Cursor on the browser address bar | BRW-001 |
| V 10:38–10:59 | Selection over the session transcript area | HRN-001 |
| V 11:12 | Reference Terminal tab (plain shell) opened briefly | UI-015, U-11 |
| V 14:50–16:23 | Task header scrolled/broken; cursor on step bar; Changes and Files tabs shown | UI-009, STP-003 |
| V 3:45, 23:30–24:35, 31:45–35:40 | Morning notes list | §2 |
| V 3:38, 35:45–37:15 | Draft prompt | §3 |

Incidental screen content deliberately **not** used: the Quartio Studio website shown in the
preview, browser tab titles, private chats and personal account messages, the unrelated Quartio
product operations visible in a second terminal, Cezar's dashboard numbers, Google Meet UI.

## 5. Self-review log

Method: after drafting, the full transcript was re-read against SPEC-001 segment by segment
(table §1), every WA note and P item was checked (tables §2–§3), and the video was re-inspected
where a statement depended on pointing.

| # | Finding | Resolution |
| --- | --- | --- |
| 1 | T 11:13 mentions an existing separate terminal; the first draft removed all non-agent terminals. | Re-checked V 11:12 (reference Terminal tab). Added UI-015 and U-11: hidden sandbox shell in the right rail. |
| 2 | Sandboxes share the VM network; parallel dev servers would collide, and the reference's Web panel previewed the app under development. | Added PRJ-010 (per-sandbox port ranges) and SPEC-002 environment variables. |
| 3 | Superadmin recovery: P(3) says a recovery code is given; T 30:22 says not recoverable for now. | P predates the call (visible at V 3:38); spoken statement wins (ACC-002, class C). |
| 4 | Browser scope: WA 13:04 per user, T 7:00 global, T 7:26 per sandbox. | Latest wins: per sandbox (BRW-002, class C). |
| 5 | Team size: P(1) 2–3, T 36:07 2–5. | Latest wins: 2–5. |
| 6 | "Settings" in P(2) vs removal at T 5:31. | Removed (ACC-010); archived tasks reachable from user menu (T 34:00 suggested a settings-like place). |
| 7 | "Limit resets" meaning unclear. | U-03: support time-window resets automatically; manual reset entitlements surfaced, not automated. |
| 8 | Dashboard is in P(2) and the reference but not discussed in the call. | Kept as analytics/overview page (ANL-003) — not removed by any correction. |
| 9 | "Coding rules will be in Yulius" (T 10:16–10:21). | Covered by DSC-001/005 and the briefing (SPEC-002 §7.6); traced in §1. |
| 10 | Product and Studio branch models differ from the transcript's dev/main. | K clarifications: delivery profiles (DLV-003/004), standard profile for new projects only. |
| 11 | Git discipline reference "this Yulius repo and our other repos" (T 21:29). | DLV-002 standard profile mirrors this repository's dev → main workflow. |
| 12 | Studio contains git-crypt-encrypted secrets and a pre-commit mass-delete guard. | SPEC-002 notes: sandboxes clone locked (secrets stay encrypted); unlocking is an explicit admin action; the preparation task follows the repo's AGENTS.md (hooks install). |
| 13 | Risk of presenting design choices as requirements. | SPEC-002 is labelled engineering assumptions; SPEC-001 A-class rows marked. |
| 14 | "whatsapp" note unexplained. | U-07: no feature invented. |

Remaining genuine uncertainties are listed in SPEC-001 §9 (U-01…U-12) and SPEC-002 §12 (R-01…R-06).

## 6. Feedback on the deployed instance (F 2026-10-07)

The founder's numbered goal after using the deployed instance, its follow-ups the same day, the
Claude-only decision of the next day, and where each item landed. Later follow-ups win over earlier items (class C).

| # | Feedback (paraphrased) | Requirements |
| --- | --- | --- |
| 1 | Per-user VS Code with profile and GitHub import. Replaced later the same day: no VS Code; a much simpler VS Code-styled editor with syntax colors and suggestions for popular languages, a clean per-user view, neither shared nor saved, no profile import. | EDT-001 (changed), EDT-003 |
| 2 | Delete a task, not only archive it. | TSK-009 |
| 3 | Leaving the agent session should return to the app and close the task view. | HRN-002 (changed) |
| 4 | Goal mode selectable and the default. | TSK-010 |
| 5 | The browser must show real DOM (or a faithful simulation), not a rendered image, and be robust. | BRW-005, ADR-09 |
| 6 | User-scoped terminals; every user has root, but as a separate user. | UI-016, U-12 |
| 7 | A welcome page in the browser. | BRW-006 |
| 8 | The coding editor still did not work. | EDT-004 |
| 9 | Usage in the accounts section. | ROT-009 |
| 10 | Changing the harness disabled by default, with explicit approval; rotation between accounts of the same provider stays automatic. | ROT-004 (changed), ROT-012 |
| 11 | A 6-digit recovery code from setup; three wrong tries end recovery; kept perfectly safe. | ACC-002 (changed), ACC-011, ADR-13 |
| 12 | Agents' mouse cursor visible in the browser, like the official plugins. | BRW-007 |
| 13 | The whole right bar hidden by default. | UI-017 |
| 14 | The "unknown tab" error. | BRW-004 (vanished tabs are not errors) |
| 15 | Links open in a normal new tab; localhost links in the internal browser. | UI-018 |
| 16 | TUI, terminal and browser as HTML text with instant copying; double-click selects in the terminal. | UI-019, BRW-005 |
| 17 | (2026-10-08, `F 2026-10-08`) Prune Codex support and provider switching now; only Claude is used; Codex returns if it gets better. Supersedes the harness approval of item 10. | TSK-001/003, ROT-001/004/007/012, ROT-005/006 withdrawn, IMP-003/004, COM-002, SKL-004, MCP-001, ADR-05/06 |
| 18 | (2026-10-08, `F 2026-10-08 UI`) All notifications in one place, behind an icon next to Status. | UI-020 |
| 19 | (2026-10-08, `F 2026-10-08 UI`) Model and effort as elegant lists like in Claude, workflow integrated under the chat, a rounded chat box; models are picked from a list, never typed; several providers may exist. | UI-021 |
| 20 | (2026-10-08, `F 2026-10-08 UI`) The Status panel must be intuitive with icons (understood in 3 seconds); sandboxes and accounts as expandable lists; each account with an initials avatar, email and plan (5x, 20x… per provider), usage as horizontal bars, accounts clearly separated. | UI-022 (email/plan need the server) |
| 21 | (2026-10-08, `F 2026-10-08 UI`) The user menu has no link to settings; add it there, with icons everywhere. | ACC-010 (changed) |
| 22 | (2026-10-08, `F 2026-10-08 UI`) New task by keyboard; ⌘N is browser-reserved, so ⌘⇧O (as ChatGPT, Claude, Gemini, Codex) was chosen. | UI-023 |
| 23 | (2026-10-08, `F 2026-10-08 UI`) The Tasks page is ugly and unreadable; research the state of the art and add filters. | UI-024 |
| 24 | (2026-10-08, `F 2026-10-08 UI`) Nothing is visible on the Git page; what information should it give me? | UI-025 |
| 25 | (2026-10-08, `F 2026-10-08 UI`) Prepare the UI and UX technically to best practice and the state of the art (audit wave 2: say what the agent needs; wave 3: technical review fixes and gaps). | UI-026, UI-027, UI-028, UI-029, UI-030, UI-031, UI-032, UI-033, HRN-005, UI-034, UI-035, UI-036, UI-037, UI-038, UI-039, UI-040, UI-041 |
| 26 | (2026-10-09, `F 2026-10-09`) Signed in as superadmin, admitted themself, signed out and signed in with GitHub in the same window: a raw "not admitted" error, then "sign-in expired" on reload. Make it work in the same window; it will be a very frequent case. (GitHub had signed the browser's current GitHub account in, not the admitted one.) | ACC-004 (changed), AC-ACC-3 |
| 27 | (2026-10-09, `F 2026-10-09`) Claude Code hit the usage limit ("You've hit your session limit · resets 10:20am"), yet the sidebar still showed the conversation as in progress; an edge case we had not foreseen. (Claude Code 2.1.292 words the limit differently; its record carries `apiError: "usage_limit_reached"` and `resetsAt`.) | ROT-010 (changed), AC-ROT-6 |
| 28 | (2026-10-09, `F 2026-10-09`) Automatic routing does not work: with two accounts used up and one working, a new task goes to an exhausted account. (Yulius knew nothing about the accounts: no usage, and the limit hits were not recognised.) | ROT-003 (changed), AC-ROT-2 |
| 29 | (2026-10-09, `F 2026-10-09`) Account statuses do not work, neither in Status nor in Settings ("usage: the provider answered HTTP 429", "Not known yet"). | ROT-009 (changed), AC-ROT-7 |
| 30 | (2026-10-09, `F 2026-10-09`) Dashboard tokens do not update in real time. (Run metrics were written only when a run ended; active agent time was never measured.) | ANL-001 (changed), AC-ANL-1 |
| 31 | (2026-10-09, `F 2026-10-09`) The chats cannot be scrolled up in any case. (Claude Code 2.1.292 draws full screen and scrolls with the mouse wheel itself; Yulius kept the mouse for text selection, so the wheel reached nothing.) | UI-019 (changed), AC-UI-11 |
| 32 | (2026-10-09, `F 2026-10-09`) The system does not show who did what: the Tasks list's By column is empty. (Your own rows hid the author, against UI-024/UI-032.) | UI-024, UI-032 (fix) |
| 33 | (2026-10-09, `F 2026-10-09`) When everything is ready for a pull request it should happen by itself; the task's icon should become the PR icon with its status, as in the preview; the Git board should move tasks on by itself; a task is done when its PR is created, tested and everything works, and the sidebar must say Done. (The agent had no way to open the PR and its briefing told it to ask; stages were reconciled only when a task was read.) | GIT-001 (changed), AC-GIT-1 |
| 34 | (2026-10-09, `F 2026-10-09`) The Git tab shows no commits and branch changes per task; it should be as in the preview. (The server had no task changes/commits API; the UI fell back to the Branch tab.) | UI-031 (changed), AC-UI-24 |
| 35 | (2026-10-09, `F 2026-10-09`) Sometimes the task view breaks and is cut on the right. (Another viewer controlled the terminal's size; this view showed it at their narrower size with an empty band.) | UI-019 (changed), AC-UI-11 |
| 36 | (2026-10-09, `F 2026-10-09`) Typing in a task is noticeably slower than in the OpenAI or Claude web apps; optimise it so it feels great. (Each echo is a round trip through the agent's TUI; the server sent small frames behind Nagle's algorithm. First step: `TCP_NODELAY`; a local composer is proposed on #29.) | SPEC-002 §7.7 |
| 37 | (2026-10-09, hardening) Every agent session showed "Auto-update failed": Claude Code's self-updater ran inside contained sessions. | SPEC-002 §7 launch contract |
| 38 | (2026-10-09, hardening; seen on the instance) An agent answered a person's message, but its reply went out as a broadcast with 0 recipients: `message_send` could not address people. | COM-005 (changed), AC-COM-2 |
| 39 | (2026-10-09, hardening; axe sweep of the mock UI) The browser and editor tab strips nested a Close button in each tab and held a non-tab in the tablist (critical); editor tabs could not be reached from the keyboard; the task preview's sessions table could not be scrolled from the keyboard. | BRW-004, EDT-001, UI-029, UI-031; web/README.md (WCAG) |
| 40 | (2026-10-09, hardening; spec-to-test review) The briefing told agents to use `scope_claim`, `scope_release`, `scopes_list`, `spec_check` and `project_context`, but the Yulius MCP served none of them; AC-COM-3 had no implementation. | COM-003, COM-004, DSC-003, DSC-009; SPEC-002 §7.4 |
| 41 | (2026-10-09, hardening; spec-to-test review) Continuing a closed task (AC-TSK-7), a manual account switch keeping model and effort (AC-ROT-5) and invisible-sandbox 404s on task and tool routes (AC-ACC-4) had no test. | AC-TSK-7, AC-ROT-5, AC-ACC-4 |
| 42 | (2026-10-09, hardening; code review) The server's own Git ran repository hooks (and honors repository config) in Git directories the sandbox's agents can write, outside containment; only the environment checkout disabled hooks. | SPEC-002 R-13 (new), R-09 |
| 43 | (2026-10-09, hardening; seen on the instance) No security headers on the UI or the API: any site could frame the signed-in app (clickjacking), no `nosniff`, no referrer policy. | SPEC-002 §8 security; docs/operations.md |
| 44 | (2026-10-09, hardening; code review) A symbolic link to `.git` inside the editor's folder opened the Git directory to the file API despite the `.git` refusal. | ADR-10, SPEC-002 §8, AC-EDT-2 |
| 45 | (2026-10-09, hardening; code review) The GitHub sign-in return path accepted backslash and control-character forms that browsers turn into another host: an open redirect right after sign-in. | ACC-003, AC-ACC-3 |
| 46 | (2026-10-09, hardening; spec-to-test review) No test checked that creating a task installs the push guard, that supervisor status transitions reach the UI, or that feedback is read back per user. | AC-DSC-5, AC-UI-3, AC-ANL-2 |
| 47 | (2026-10-09, hardening; code review) Open WebSockets outlived logout, disabling and revoked access: the event stream cached sandbox visibility for its lifetime and no socket re-checked the session. | ACC-008, AC-ACC-5, AC-ACC-3 |
| 48 | (2026-10-09, hardening; self-review of #40/#41) A task waiting for approval showed Done; a merged PR fell back to "Your turn"; required checks not yet reported counted as tested; commits after the PR opened were never pushed and a failed push never retried; diff counts were wrong past the review budget; untracked files were unbounded; merged deliveries were re-read forever. | GIT-001, UI-024, UI-031, AC-UI-24 |
| 49 | (2026-10-09, hardening; code review, reproduced) Briefing, `project_context` and Stop-gate document paths followed directory links out of the worktree, so a contained agent could read host files through the server. | SPEC-002 §8 containment, COM-004, DSC-003 |
| 50 | (2026-10-09, hardening; self-review of #32, reproduced) A failed usage refresh wrote back the account as read at the start of the round, undoing a limit a running task observed meanwhile, so routing used the exhausted account again. | ROT-003, ROT-009, AC-ROT-2 |
| 51 | (2026-10-09, hardening; self-review of #53, reproduced) A repeated scope claim messaged and woke the overlapping task every time; a malformed scope_release freed every claim; claims per task were unbounded. | COM-003, AC-COM-3 |
| 52 | (2026-10-09, hardening; goal item c) Parsers fed by untrusted input had hand-written cases only; randomized tests (no defect found) now cover the diff parser, the spec format, scope patterns, the transcript observer and usage JSON. | AC-UI-24, DSC-003, COM-003, ROT-009 |
| 53 | (2026-10-09, hardening; self-review of #37/#42, reproduced) Viewers who opened a task after a full-screen app started could not wheel-scroll it (snapshot without alternate screen and mouse modes); a wheel turn took control and resized the terminal; fitted text could lose its bottom rows. | UI-019, AC-UI-11, TSK-006 |
| 54 | (2026-10-09, hardening; self-review of #51, reproduced) Closing an editor tab with Delete moved focus into the next file, so the next Delete edited it; held keys repeated; focus was lost after some closes; a clicked browser tab could keep focus while its page loaded. | EDT-001, BRW-004, UI-029 |
| 55 | (2026-10-09 evening, `F 2026-10-09`) "In editor I should be able to select whether to break lines or not." | EDT-005 (new), AC-EDT-4 |
| 56 | (2026-10-09 evening, `F 2026-10-09`) The right tool rail is "a little broken" (screenshot: two highlighted items whose pills ran past the rail's right edge, the Messages count against it). (Reproduced: a panel opened in a wider window kept its width when the window narrowed, so the layout grew wider than the window and cut the rail's right side off; the Messages count did not fit its pill; hover and the open tool looked alike; on phones the top bar ran under the rail.) | UI-042 (new), AC-UI-36 |
| 57 | (2026-10-09 evening, `F 2026-10-09`) "Something is broken with accounts: as superadmin I should be able to see all sandboxes but I see none." (By design until now: every access decision required a GitHub identity, which the password superadmin never has, U-06; not a regression of #55/#64/#67.) The superadmin owns the instance and must see and open every sandbox with full access (#95). | ACC-012 (new), AC-ACC-8, ACC-007, ACC-009, U-06 (superseded); SPEC-002 principle 3, §8 |
| 58 | (2026-10-09 evening, `F 2026-10-09`) The terminal should work instantly; a terminal is the user's and a user creates many; per user per sandbox there can be many terminals, different between sandboxes and users. (The Shell panel showed only an "Open shell" button; "+" replaced the one terminal; nothing listed a user's terminals, so a reload lost them, and a restart ended them.) | UI-015, UI-016 (changed), AC-UI-7, AC-UI-8, AC-UI-37, SPEC-002 §4, §7.2, §8 |
| 59 | (2026-10-09 evening, `F 2026-10-09`) When I open Claude the font size changes three times and it loads for a while; make the Claude view much more user friendly. (Measured: the terminal opened at its own size with a fallback font, then the first state fitted another viewer's size, the agent redrew at the new size and the web font arrived; a read-only row flickered in before the first state; the attention bar resized the terminal every turn; every reload downloaded the UI again.) | UI-026 (changed), UI-043, AC-UI-38, SPEC-002 §7.7 |
| 60 | (2026-10-09 evening, `F 2026-10-09`) "We need to do the browser somehow completely differently. Even if we have this mirror thingy, it works terribly when we need to do something real. It should be rendered fully on the user's side, but we can use the VM to get around iframe problems. It should just work as a part of the page. Deny recurrence: the user can't open the VM view in the browser because it'll explode. We still need that cursor thingy." (The user's view was a replay of the server's Chrome; nothing stopped it from opening Yulius itself.) | BRW-005/006/007 (changed), BRW-008 (new), AC-BRW-4..8; SPEC-002 ADR-14 (ADR-09 narrowed) |
| 61 | (2026-10-10, `F 2026-10-10`) "The browser is fucked up. It's slow as fuck — on scrolling or typing — and clicking on a captcha doesn't work. The current approach seems wrong. Fix. You can use a completely different approach because the bugs seem to be in the foundations." (On production he used the sandbox Chrome through the DOM mirror; #105's preview origins had no certificate. Every wheel turn and key was a replay round trip; the Chrome was headless with `navigator.webdriver` true, and the mirror cannot record or click cross-origin captcha frames. The need: drive the agent's own Chrome through captchas and sign-ins, then watch it.) | BRW-005..008 (changed), BRW-009, BRW-010 (new), AC-BRW-4..7 (changed), AC-BRW-9..12 (new); SPEC-002 ADR-15 (ADR-09 superseded for the user's view, ADR-14 narrowed), §3, §8, §9, §10, R-05 |
| 62 | (2026-10-10, `F 2026-10-10`, item 2) On the bottom of the Messages panel (a composer "Message #1 — delivered to the agent like a task message", "Ctrl/⌘ + Enter to send", Send): "here should be only messages received by other sessions/task. User sends messages in Claude." (The composer posted a person's message into the agent's inbox, a slower second way to talk to an agent that has a terminal; nothing else used the endpoint.) | COM-005 (changed), AC-COM-2; SPEC-002 §14, §15; #110 |
| 63 | (2026-10-10, `F 2026-10-10`, item 8) "bruh", at the Git page of Quartio-Studio/test-app showing "GitHub: checks cannot be read: GitHub: GitHub refused the request (HTTP 403): Resource not accessible by integration". (The installed App has Checks: read but not Commit statuses: read, which the manifest never requested; the combined-status read failed the whole checks read, every pull request read "unknown", which the gate treated as "running", and the note repeated "GitHub" three times on every visit.) | GIT-001 (changed), AC-GIT-1; SPEC-002 ADR-08, §10, §14, §15; `docs/operations.md` (App permissions); #111 |
| 64 | (2026-10-10, `F 2026-10-10`, item 10 c/d of the "final state" list) "We want to use jev. On starting new tasks, Jev should detect whether it should be split to more tasks … gently inform user … propose split (claude credentials) … keep tasks discipline: (i) new feature in the current scope → run parallel task OR schedule the goal right after this one (ii) if the new prompt rules out some scope, this ruled out scope should be declared as skipped … and then the new task should be scheduled right after (iii) user can add context … if the changes are huge, the current task should be reverted, task canceled and new task started … it should be consistent"; "chaining tasks — we want to schedule tasks to run right after the current one"; refined the same day: one task when the thread is consistent, "ux work" and "backend work" rather than 10 tasks; "do not overcomplicate this jev thingy". (Every task was one task, every typed prompt reached the agent, and nothing could wait for another task.) | TSK-011, TSK-012, TSK-013, HRN-006, STP-005 (new), AC-TSK-10..12; SPEC-002 ADR-16, §5, §6.1, §7.5, §9, §10 |
| 65 | (2026-10-10, `F 2026-10-10`, items 1, 3, 6 and 7 of the same day) "Make the bars resizable horizontally" (the sidebar, the editor's explorer, the tool panel); "I'd like to have main screen with sandboxes as default view"; "when it's smaller buttons are overlapping" (the Tasks header, state pills and table headers); "here is some space at the right" (Workflows). (Only the panel had a handle, invisible until hover; `/` redirected to the last used sandbox; the Tasks state pills never wrapped and an origin chip spilled over the Status column, and a panel next to a 300 px sidebar left the centre about 260 px wide; `.page-inner` had a maximum width without auto margins.) | UI-044, UI-045, UI-046, UI-047 (new), AC-UI-39..42 |
| 66 | (2026-10-10, `F 2026-10-10`, item 10b) "We want to reduce management of subscriptions — priority etc. … we add claude subscriptions … 5h limit and weekly limit … run multiple tasks at the same moment but we don't want to manage subscriptions … the most efficient mechanism of switching … maximally use all our resources … monitor when we'll need to buy more subscriptions … pausing tasks should be gentle: paused when 98% is used and nothing is left atm, and resumed gently when the first subscription arrives … done by our harness only and do not involve prompting claude … invisible for the agents." (Before: priorities and an enabled switch ordered the accounts, nothing preferred capacity about to reset, the threshold was 99 %, per-model caps were dropped, a resume typed a prompt into the agent, a task with no capacity at start failed, nothing forecast the need for subscriptions.) | ROT-003, ROT-004, ROT-009, ROT-010, ROT-012 (changed), ROT-013 (new), AC-ROT-2/6 (changed), AC-ROT-9..14 (new); SPEC-002 ADR-17, §5, §6.1, §6.3, §7.5, §9, §10, R-04, R-14 |
| 67 | (2026-10-10, `F 2026-10-10`, item 5) "Still a few terminal bugs … it's not full size. It's 'someone else is controlling this width' thingy but each user should be able to control it on their side … ctrl c ctrl v doesnt work in the terminal for some reason"; then "clicking on this should open editor" (a spec path in Claude Code's output) and "other files in claude should be clickable and open editor, as links open websites". (The last typist kept the size even from a background tab; the fitted view was capped at 1.6×, never checked, and could cut off the bottom rows; xterm sent ^C and ^V for Ctrl+C and Ctrl+V; only web links were links.) | UI-018, UI-019, TSK-006 (changed), AC-UI-10, AC-UI-11, AC-TSK-6, SPEC-002 §7.7 |
| 68 | (2026-10-10, `F 2026-10-10`, item 10(a)) "When the task's goal is achieved and none left shell is running, the task should be marked as finished (with green 'v' icon). Atm it keeps running." (Production task #1: Claude Code showed "✓ Goal achieved"; the sidebar kept it under Working with a spinner for 45+ minutes. Found: nothing read Claude Code's `goal_status` records, and each Yulius restart re-read the transcript and set the replayed prompts' `working` again, which no replayed `end_turn` undid.) | TSK-014 (new), AC-TSK-13, GIT-001, UI-043 |
| 69 | (2026-10-10, `F 2026-10-10`) "Also do drag and drop for files to both editor and claude (like images but also pdfs etc)." (The files API took JSON text only, the terminal only keystrokes, and a file dropped anywhere made the browser open it in place of the app.) | EDT-006, UI-048 (new), AC-EDT-5, AC-EDT-6, AC-UI-43 (new); SPEC-002 §4, §7.9, §8, §9, §14 |
| 70 | (2026-10-10, later, `F 2026-10-10`) "Browser is slow as fuck. Especially when scrolling and moving. Improve this dramatically." Then, live on production, the panel showed "input arrives faster than the display takes it" while moving. (Measured at the client in Poland: #108's figures were taken with the viewer on the VM; over the real link the wheel queued up behind one CDP round trip per event, the client skipped to the next keyframe after every burst, frames piled up in the tunnel's or Caddy's buffers without flow control, and pointer moves filled the X thread's queue. #122.) | BRW-009 (changed), AC-BRW-12 (changed), AC-BRW-13 (new); SPEC-002 ADR-15 ("Over the user's link"), R-05, §14 |
| 71 | (2026-10-10, evening, `F 2026-10-10`, final stage items 1(b) and 2; #136) "Make sure mcp works. When I click Add mcp server, nothing happens. MCP configured for Yulius should work on every subscribtion (and harness after we add more)." "Ajust git, skills and workdlows … git should be presentation layer striictly because all reposnisbilities will ba handled by agents. Skills should be reduced, we need some default workflows." (Found: on an MCP page without servers the empty state's Add server called a function the page no longer had, a `ReferenceError`; runs already got the servers per run, independent of the account, but nothing pinned it; the Git page, the delivery dialog and the task strip offered merge, release, deploy, verify and open-PR buttons; Skills were three filtered sources; the two seeded workflows were editable rows.) | UI-038, MCP-003 (changed), MCP-004 (new), GIT-001, GIT-002 (changed, class C), UI-025, UI-030, UI-034, UI-037 (changed), SKL-006, AUT-006 (new); AC-MCP-2, AC-GIT-2, AC-SKL-3, AC-UI-18, AC-UI-23, AC-UI-28, AC-UI-31, AC-UI-32 (changed), AC-MCP-3, AC-AUT-5 (new); SPEC-002 §7.2, §7.4, §7.10, §8, §14 |
| 72 | (2026-10-10, evening, `F 2026-10-10`, "final stage" list, items 1(c), 1(e) and 2) "Improve the UX before it behaved weirdly tbh … I'd like to have better loading (some animation), different layout etc" (the split card above the prompt box with monospace prompts and three equal buttons, only a spinner in the send button meanwhile); "remove enter+shift thingy, enter by default should do next line and arrow should send task. We want to have working markdown formatting there, like when i do lists etc."; "in task view, we want goal and workflow to have auto mode by default (Jev decides)"; "featurs must have specs but research not, questions about the product also not … robust but flexible, first principles". | UI-021, TSK-010, TSK-011 (changed), TSK-015 (new), AC-UI-13 (changed), AC-UI-44, AC-TSK-14 (new); SPEC-002 ADR-16 item 5, §5, §9, §14; #135 |
| 73 | (2026-10-10 evening, `F 2026-10-10`, final stage item 1(a)) "weird in subscriptions section: `Usage: the provider refused this account's token for usage (HTTP 403); it refreshes on the next run or needs a re-login`. Also, not sure if you understood: this pausing thingy is only when no subscription is left. Tasks are subscription-agnostic and the subscription should change automatically under the hood." (Production: all three subscriptions had that error and no usage at all; the `setup-token` sign-in lacks the usage endpoint's `user:profile` scope, so the pool never had readings. The task menu offered "Switch account…" with an account picker, the task strip showed the account, and "waiting for subscription capacity" read as the task's own subscription running out. #143.) | ROT-007, ROT-009, ROT-010, HRN-005 (changed), AC-ROT-15, AC-ROT-16 (new); SPEC-002 ADR-17, §7.2, §7.5, R-04, §14 |
| 74 | (2026-10-10 evening, `F 2026-10-10`, final stage item 1(c)) "Tasks are not synchronised with the session — when I stop claude, task status should change. Make sure all those jev things we build work as intended, they should catch messages sent to claude as well." (Production: Esc/Ctrl+C during a turn writes "[Request interrupted by user]" with no Stop hook, which Yulius read as a new prompt, so the task stayed Working; a killed process read as "crashed"; a held turn that was interrupted or dropped by the CLI kept the task Paused; typed prompts into paused tasks were not checked, and a passing check left no trace. #144.) | HRN-007 (new), HRN-006 (changed), AC-TSK-14 (new); SPEC-002 §6.1, §7.2, §7.5, §14 |
| 75 | (2026-10-10, evening, `F 2026-10-10`, "final stage" item 3(b), and the follow-up "features must have specs but research not, questions about the product also not etc. It should be robust but flexible, first principles") "Make sure todos declared by every task takes similiar amount of time: 9/10 should mean 90% etc. Second, make sure they'll keep arriving uniformly to avoid 0%->100% jumps when everything is touched a little bit. Analyse existing om skills: throw out om-verify-in-repo. Rephrase them to avoid connections to manual mode … Make them universal. Add small bar that they come from openmercato … Implementing feature should look always the same: spec (inspired by OM), implementation, review, fixes, tests, fixes, merge to dev and deploy … merge to main and deploy to prod … On first prompt of fresh project or on onboarding the agent should programmatically create very robust and SOTA infra … The agents must maintain consistent docs … Sessions as docs … No acceptance by human"; later the same evening: no GitHub Actions at all (onboarding translates them into Yulius CI and deletes them) and explicit Git hygiene (no stash, atomic commits, cleanup). Bartek's N2/N3 delivery playbook as input. (The Feature workflow seeded phases, the step tools accepted any shape and any number of open steps, the Stop gate demanded a spec for every change, the briefing said people merge, the onboarding asked the user, and the Open Mercato skills were byte-for-byte copies tied to their harness.) | STP-006..008, HRN-008, DSC-013..015, SKL-007, DLV-008 (new), DSC-008, DSC-010, SKL-003 (changed), §7.2, §7.3, AC-STP-4..6, AC-DSC-6..9, AC-DLV-8 (new), AC-SKL-2 (changed); SPEC-002 ADR-19, §2.5, §4, §5, §7.4–7.6, §14; #139 |
| 76 | (2026-10-10 evening, `F 2026-10-10`, final stage items 1(e) and 2) "Add update method. The repo is here https://github.com/Quartio-Studio/yulius and always will be. When new thing is on prod, users who has installed it should be asked to update (superadmins). Do not do autoupdate at the moment. All tasks and everything should be paused for the update and resumed right after without agents knowing etc. but also users should be able to do that because developing yulius using yulius, our vm will be prod where we'll do final tests." and "On installing Yulius user should be prompted for Jev key and it should be managed by superadmin later." (Before: deploys were manual on the VM — copy a tarball, build, install, restart in the middle of agents' turns; Yulius did not know its commit; the Jev key was a file read at start, never asked for at setup and not manageable in Yulius.) | OPS-006..OPS-009, ADM-001, ADM-002 (new), AC-OPS-6..9, AC-ADM-1/2 (new); SPEC-002 ADR-18 (§2.5), §3, §4, §5, §6.8, §7.5, §8, §9, §10 |
| 77 | (2026-10-10 evening, `F 2026-10-10`, final-stage item 3a) "We want to have 2 sections for storing secrets … team secrets: passwords to shared mails, infra etc. … for agents: API keys, also passwords etc. Agent can access them but never see them … use some ready library … it should be very safe … in early startups the permission layer is flat … secrets are sent on WhatsApp, mail … we don't have a good way to pass credentials to agents without leaking them." Follow-up the same evening: "maybe just check all input for the exact strings we have and redact them?" (Before: no shared store; credentials reached agents only through files, the environment, MCP server settings or prompts, all readable by the agent and kept in its transcript.) | VLT-001..009 (new), AC-VLT-1..8, U-13; SPEC-002 ADR-20, §4, §5, §7.2, §7.4, §7.5, §8, §9, §10, R-15, §14; runtime contracts SEC-004; #141 |
| 78 | (2026-10-10, `F 2026-10-10`, final stage, item 3(b) and a follow-up) "All tests and CI/CD should be 100% local, using local blacksmith instance. Zero paid github features. Keep github hygiene clean" and "onboarding should contain deleting GitHub Actions completely and using only blacksmith" (Yulius's own CI on our VM); "merge them autonomously, then delete branches and worktrees … no stale branches". | DLV-006, DLV-007 (SPEC-002 ADR-21) |
| 79 | (2026-10-10, `F 2026-10-10`, final stage, item 1(d)) "Look for other things that are inconsistent at the moment." An audit of the mock UI, the API and the docs found 56 items (#147); the unowned ones are fixed without new requirements (copy, states, keyboard, layout, mock, docs). The rest went to the owning issues. Its leftovers — "needs you" counted one way on the server and in the sidebar, one icon per waiting state, one pattern for headers, empty, failed and missing pages, the sidebar outside a sandbox, one name for subscriptions, scopes and isolation, and a pull request that makes a task Done only with its plan complete — became one requirement. On 2026-10-11 (`F 2026-10-11`): "ux is a little broken, fix" on the Secrets page and "when i click outside any popup i want it to close" — the same requirement: the add action in the header, the page spacing, and dialogs closing on a click outside. | Existing UI, ACC and DLV rows; UI-050 (AC-UI-45); TSK-014 and GIT-001 (open plan steps) |
| 80 | (2026-10-10, `F 2026-10-10`, final-stage item 4) "Make sure isolation is good enough - think how sandboxes should be isolated etc, we want to have best practices here. Also we'd like to run sudo apt update etc by users in case we need some weird library." (Measured on the VM: every sandbox shared the host's network namespace, so it reached Yulius's listeners, Caddy's admin API on 127.0.0.1:2019, every sandbox Chrome's DevTools port and other sandboxes' dev servers; there were no limits and no syscall filter; `apt` could not write the read-only host system and a single-uid namespace cannot `chown` to `_apt`.) | PRJ-011, NFR-007 (new), AC-PRJ-6, AC-NFR-8, AC-NFR-9 (new), U-12 (resolved); SPEC-002 ADR-22, §3, §4, §8, §9, §10, §11, R-07, §14; #142 |
| 81 | (2026-10-10 evening, `F 2026-10-10`, "final stage" item 3b) "We'd like to onboard repo from my computer using cli with its claude sessions. Those claude sessions should go on server as well. I want to be able to see them in read only mode as any other session. It should be accessible from the panel. Bartek will be able to add his session using cli and any other person as well. Tasks on server can directly lookup to other tasks transcripts directly and also the onboarded sessions. Sessions as docs. It should be a natural practice for a task when it doesnt know how something was implemented. Make this cli beautfiul as well with progress bar etc. UX is important. … Remember about onboarding - user can't start a task until we know it's ready for yulius." (Before: `yulius import` made every session an archived task, nothing showed a transcript, agents could not look into earlier sessions, transcripts were uploaded with their secrets, the CLI printed plain lines, continuing a task was not gated by onboarding and the preparation task waited for approval. #137.) | IMP-004 (changed), IMP-007..010, SES-001..005, UI-049 (new), U-08, J4, §7.5; AC-IMP-4/6 (changed), AC-IMP-7..10, AC-SES-1..4 (new); SPEC-002 §4, §5, §6.5, §6.6, §6.10, §7.4, §7.6, §9, §10, §14 |
| 82 | (2026-10-09, `F 2026-10-09`, decision on #88 from the design note in #44; urgent with `F 2026-10-10`'s "after this iteration we'll redeploy yulius and use it for us") Behind the reverse proxy every sign-in came from `127.0.0.1`, so one global bucket of 5 attempts a minute let anyone keep the superadmin's password sign-in locked. Decided: the proxy adds the client address and a shared secret, always overwriting both; Yulius keys on the address only when the secret matches in constant time, else on the peer; the secret is server configuration never readable from sandboxes; ship Caddy and nginx snippets. (Checked: no other limit is per client: recovery codes are counted instance-wide by design, setup needs the one-time token, device approval a session. Isolated sandboxes cannot reach the public listener, ADR-22; runs without isolation can, so the secret stays as defence in depth.) | ACC-013 (new), AC-ACC-9 (new); SPEC-002 §8, §9, §14, ADR-22 |

## Changelog

- 2026-10-11: the Secrets page layout and dialogs closing on a click outside (§6 item 79, `F 2026-10-11`).
- 2026-10-11: the consistency pass's leftovers (§6 item 79, `F 2026-10-10`, #147): UI-050, AC-UI-45; TSK-014 and GIT-001 on open plan steps.
- 2026-10-11: per-client sign-in limits behind the reverse proxy (§6 item 82, `F 2026-10-09`, #88).
- 2026-10-10: onboarding from a computer with Claude sessions, sessions as docs and the onboarding gate (§6 item 81, `F 2026-10-10`, #137).
- 2026-10-10: sandbox isolation and `apt` per sandbox (§6 item 80, `F 2026-10-10`, #142).
- 2026-10-11: Yulius-run CI and repository hygiene; the consistency audit (§6 items 78–78, `F 2026-10-10`).
- 2026-10-10: the secrets vault (§6 item 77).
- 2026-10-10: updates from main without disturbing agents, and the Jev key in setup and settings (§6 item 76, `F 2026-10-10`).
- 2026-10-10: the delivery discipline — honest steps, specs where they matter, one autonomous
  pipeline, infra bootstrap, docs first, adapted skills (§6 item 75, `F 2026-10-10`, #139).
- 2026-10-10: subscription-agnostic tasks and usage from the sessions (§6 item 73, `F 2026-10-10`, #143).
- 2026-10-10: the task follows its session however it stops (§6 item 74, `F 2026-10-10`, #144).
- 2026-10-10: the composer's keys, Markdown, loading and layout, and Auto mode and workflow (§6 item 72, `F 2026-10-10`, #135).
- 2026-10-10: MCP on every run and Add server, Git as a presentation layer, fewer skills and
  default workflows (§6 item 71, `F 2026-10-10`, #136).
- 2026-10-10: the streamed browser over the user's link (§6 item 70, `F 2026-10-10`, #122).
- 2026-10-10: dropped files for the editor and Claude (§6 item 69, `F 2026-10-10`).
- 2026-10-10: a task whose goal is achieved is Done (§6 item 68, `F 2026-10-10`).
- 2026-10-10: the terminal's size per viewer, copy and paste, and file links (§6 item 67, `F 2026-10-10`).
- 2026-10-10: Claude subscriptions as one self-managing pool (§6 item 66, `F 2026-10-10`).
- 2026-10-10: resizable columns, the home screen, headers without overlap, centred pages (§6 item 65, `F 2026-10-10`).
- 2026-10-10: task intake with Jev and chained tasks (§6 item 64, `F 2026-10-10`).
- 2026-10-10: the Messages panel without a composer and checks the GitHub App cannot read (§6
  items 62–63, `F 2026-10-10`).
- 2026-10-10: the streamed sandbox browser (§6 item 61, `F 2026-10-10`).
- 2026-10-09: the live browser panel and no recursion (§6 item 60, `F 2026-10-09`).
- 2026-10-09: user terminals and how the task terminal opens (§6 items 58–59, `F 2026-10-09`).
- 2026-10-09: the superadmin sees every sandbox (§6 item 57, `F 2026-10-09`).
- 2026-10-09: editor word wrap toggle and ⌥Z / Alt+Z (§6 item 55, `F 2026-10-09`).
- 2026-10-09: the tools rail keeps its items inside it and stays in the window (§6 item 56, `F 2026-10-09`).
- 2026-10-09: tab keys keep focus on the strip and ignore key repeat (§6 item 54, hardening).
- 2026-10-09: terminal follow-ups: full snapshot for late viewers, scrolling without taking control, fit by rows too (§6 item 53, hardening).
- 2026-10-09: randomized parser tests (§6 item 52, hardening).
- 2026-10-09: scope claims: no repeated messages, strict release, at most 50 per task (§6 item 51, hardening).
- 2026-10-09: a failed usage refresh records only its error (§6 item 50, hardening).
- 2026-10-09: repository documents are read only inside the worktree (§6 item 49, hardening).
- 2026-10-09: follow-ups to the automatic pull request and the task review (§6 item 48, hardening).
- 2026-10-09: open sockets re-check the session and the event stream re-checks visibility (§6 item 47, hardening).
- 2026-10-09: tests for the task push guard, status events and feedback readback (§6 item 46, hardening).
- 2026-10-09: the sign-in return path stays on the site (§6 item 45, hardening).
- 2026-10-09: the file API refuses paths that resolve into `.git` (§6 item 44, hardening).
- 2026-10-09: baseline security headers on every public response (§6 item 43, hardening).
- 2026-10-09: server-side Git ignores repository hooks (§6 item 42, hardening; config is #54 part B).
- 2026-10-09: tests for continue, manual switch and invisible-sandbox access (§6 item 41, hardening).
- 2026-10-09: the announced scope, spec-check and project-context tools are served (§6 item 40, hardening).
- 2026-10-09: accessible browser and editor tabs, focusable sessions table (§6 item 39, hardening).
- 2026-10-09: agents answer people (§6 item 38, hardening).
- 2026-10-09: Claude Code's self-updater off in agent sessions (§6 item 37, hardening).
- 2026-10-09: typing latency, first step (§6 item 36).
- 2026-10-09: a terminal sized by another viewer fitted to the width (§6 item 35).
- 2026-10-09: the task review's changes and commits served (§6 item 34).
- 2026-10-09: automatic pull requests and a self-moving Git board (§6 item 33).
- 2026-10-09: the By column on every Tasks row (§6 item 32).
- 2026-10-09: terminal scrolling from feedback on the deployed instance (§6 item 31).
- 2026-10-09: live run metrics from feedback on the deployed instance (§6 item 30).
- 2026-10-09: routing past limited accounts and account usage from feedback on the deployed instance (§6 items 28–29).
- 2026-10-09: the usage-limit status from feedback on the deployed instance (§6 item 27).
- 2026-10-09: the sign-in refusal flow from feedback on the deployed instance (§6 item 26).
- 2026-10-08: the notification center, the composer, the Status panel and Settings from the UI review (§6 items 18–21).
- 2026-10-08: the Claude-only decision (§6 item 17).
- 2026-10-07: feedback coverage (§6).
- 2026-10-07: initial traceability review and self-review log.
