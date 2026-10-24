---
yulius-task: 50d1de50-7c3b-42e9-bd48-27fd7bf1d8ac
status: done
---

# SPEC-299: A team secret's Value keeps pasted line breaks

Status: done
Date: 2026-10-24
Owner: Yulius task #285 (fixes #592)
Scope authorization: task #285, opened by task #283 for issue #592 (reported by task #179). No
new issues: findings go to #592 as comments.

## Goal

A person pastes a multi-line value (a PEM key, recovery codes) into a team secret's Value, in
"Add team secret" or "Edit", and the vault stores it with its line breaks, exactly as pasted. The
field stays masked while typed and pasted. Show displays the value on its lines; Copy (in the
dialog and on the card) gives back the original text.

## Context

- Symptom (#592): the pasted value is stored on one line; Copy gives back the flattened text.
- Root cause: `teamForm` in `web/assets/pages/secrets.js` renders Value as
  `<input type="password">`, a single-line control. The HTML value sanitisation of a single-line
  input strips U+000A and U+000D (Chrome first turns a pasted line break into a space), so the
  page never reads a line break and `values()` sends the flattened text. The server keeps line
  breaks: `vault::field(.., trim = false)` (`yulius-app/src/vault/mod.rs`) refuses only NUL and
  oversize values, called from `vault/team.rs` on create and update.
- A revealed value is a `code.secret-value` (`maskedValue`), whose style has no
  `white-space: pre-wrap`: a stored multi-line value would also show on one line.
- Agent secrets (#591, SPEC-193, ADR-0156) store a pasted key with literal `\n`, because an agent
  secret never spans lines. That form is wrong here: team secret values are read by people, and
  Copy must return the text they pasted.
- Why tests missed it: the team e2e tests fill Value with one-line text only.

## Requirements

- TLB-001: A paste holding line breaks (LF, CRLF or CR) into a team secret's Value, in Add or
  Edit, stores the value with those exact line breaks; text typed around it is kept.
- TLB-002: The Value field stays a masked password field while typed or pasted: the value is never
  in the page's markup.
- TLB-003: Show displays a stored value on its lines (wrapping long ones); Copy, in the dialog and
  from the card, writes the stored value unchanged.
- TLB-004: Pastes without line breaks, and agent secrets' Value (SPEC-193), behave as before.

## Plan

- [x] Regression e2e test for LF and CRLF pastes that fails on dev
- [x] Pasted line breaks are kept in a team secret's Add and Edit, masked
- [x] Show displays a multi-line value on its lines; Copy returns it
- [x] Decision record, docs and changes entry
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green and the spec's Evidence written

## Acceptance criteria

- Pasting `…\r\n…\r\n` into Add's Value stores exactly that text (mock vault), and `…\n…` pasted
  over a selection in Edit stores exactly the text typed before, the paste and the text after.
- During and after the paste the Value field is `type="password"` and the value is not in
  `document.body.innerHTML`.
- After a two-factor code, Show value renders the value over several lines, and its
  `textContent` and both Copy buttons give back the stored text.
- A one-line paste is left to the browser (not prevented); the agent secret test of SPEC-193
  still passes.

## Test plan

- e2e (`e2e/tests/secrets.e2e.ts`): "a pasted multi-line team secret value keeps its line
  breaks, masked, and Show and Copy return them" covering TLB-001..004 with a CRLF paste in Add
  and an LF paste in Edit (paste dispatched as a `ClipboardEvent` with a `DataTransfer`, as
  SPEC-193's test does), Show, the dialog's Copy and the card's Copy (clipboard stubbed), and axe.
- Run first on the unchanged page to show it fails (the stored value has no line breaks).
- The SPEC-193 agent-secret test and the rest of `secrets.e2e.ts` stay green.

## Decisions

- Keep `<input type="password">` and carry each pasted line break as a private-use character
  (U+E00A for LF, U+E00D for CR) that the form turns back into the break on save. Recorded in
  [ADR-0214](../../docs/decisions/0214-a-team-secret-value-carries-pasted-line-breaks-in-its-password-field.md).
  Rejected: a `<textarea>` masked with `-webkit-text-security` (non-standard, its text is read
  aloud and selectable, password managers ignore it); a "multi-line" toggle that unmasks; #591's
  literal `\n` (changes the value people copy back).
- Line breaks are stored as pasted, CRLF included: Copy must return the original text.

## Review

Independent review in a fresh session (read-only subagent, skill `om-code-review`, diff
`e8cb3398..HEAD` and this spec), two rounds, both APPROVE, every finding own code:

- Round 1 at e1e2c6f7: minor, the paste helper's `inMarkup` check could never fail (a field's
  value is never in the markup): removed, the `password` type carries the masking proof. Nits: the
  ADR counted a CRLF as one dot (now "each LF and CR, a CRLF as two"); a short line in
  product.md (reflowed); this spec's test title (corrected); drag and paste-less insertion still
  lose line breaks (kept out of scope, written in ADR-0214's Consequences: the hint promises a
  paste only).
- Round 2 at b9872ad0: all answered; one cosmetic nit (long lines): prose lines wrapped, the
  lines left over 100 characters hold a link or a test name. The reviewer ran the new test 5
  times (the secrets file once per round and four filtered reruns, all passed) and showed it fails
  with the old `secrets.js` (first paste not taken) and with the old CSS (`lines: 1`).
- Round 3 at a9197aa9: request changes until a green test stage at the head is recorded (the local
  gate below had one failing test) and two statements here are corrected; no code change asked.
  Answered with the PR's CI run at a9197aa9 and the corrections in the next commit.

## Evidence

- Regression first: `npx e2e run tests/secrets.e2e.ts --grep "multi-line team secret"` on the
  unchanged page (commit bc6e33a7) failed: `expected {"prevented":false,"type":"password",…} to
  equal {"prevented":true,…}`. With only the JS fix it failed at Show (`lines: 1`, expected 3);
  with the CSS it passed.
- `cd e2e && npm ci --ignore-scripts && E2E_TELEMETRY_DISABLED=1 npx e2e run tests/secrets.e2e.ts
  tests/api-tokens.e2e.ts`: 31 passed; after the review fixes `tests/secrets.e2e.ts`: 25 passed.
- Yulius CI on PR #884 at a9197aa9 (run f9c792cf): **quality passed** (`scripts/dev ci`, the
  whole gate with every test) in 12 min, **e2e passed** (the whole suite) in 17 min, security
  skipped (Vidoc out of quota, below).
- Local `python3 scripts/bootstrap.py && scripts/dev ci` at e1e2c6f7, before: every stage passed
  but the test stage, 2599 of 2600 tests. The one failure,
  `leftovers::kept_files_of_a_removed_sandbox_are_listed_and_deleted_on_request`
  (`kept[0].bytes` was `null` while the same view listed the sandbox's item with 196608 bytes),
  is in the disk-leftovers code this change does not touch (no Rust in the diff); alone it passed
  5 of 5 (`cargo nextest run -p yulius -E 'binary_id(yulius::main) &
  test(/^leftovers::kept_files_of_a_removed_sandbox/)'`), and in the CI run above. Reported on
  #592 and in the PR (this task opens no issues).
- `python3 scripts/check_repository.py` and `--ui`: passed.
- Security: `yulius security-scan` (the scanner `scripts/dev security` runs, without a cargo
  build beside the gate) exited 2, not run: Vidoc answered HTTP 429, this key has used its
  50,000,000 tokens. The three changed code files were not scanned; this is unrun, not passed.
- Not tested: Firefox and Safari (the suite runs Chromium only; `execCommand('insertText')` and
  `setRangeText` are in both), a real OS clipboard (pastes are dispatched `ClipboardEvent`s, as in
  SPEC-193's test), and the Rust server (unchanged: `vault::field` keeps line breaks, as read).

