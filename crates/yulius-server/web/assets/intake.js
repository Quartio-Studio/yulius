// Task intake in the UI (SPEC-001 TSK-011..013, SPEC-002 ADR-16): the composer's split check
// and card, the "Run after" choice, and the card of a prompt held by the scope check.
// Every API path is a literal at its call site (tests/ui_contract.rs checks them).

import { enc } from './api.js';
import { h, icon, iconButton, setChildren, toast, errorToast, confirmDialog, skeleton } from './dom.js';
import { allows, isLive } from './status.js';

/** Prompts with fewer words are one task without asking (the server's rule, SPLIT_MIN_WORDS). */
export const SPLIT_MIN_WORDS = 40;
/** How long the composer waits for the split check after Enter before creating the task. */
const CHECK_WAIT_MS = 4000;
/** How long a proposal may take (one headless Claude run, bounded at 60 s on the server). */
const PROPOSAL_WAIT_MS = 75000;

/** Tasks a new task can run after: those of the sandbox that have not ended. */
export function chainCandidates(ctx, sandbox) {
  return (ctx.store.tasksOf(sandbox.id) || [])
    .filter((t) => !t.archived && (isLive(t.status) || t.status === 'chained' || t.status === 'prepared'))
    .sort((a, b) => b.number - a.number);
}

/**
 * Asks whether a new task's prompt mixes separate workstreams. Resolves to true only for a
 * confident "several"; short prompts, a slow or failing check and no Jev resolve to false, so
 * the task is created as asked.
 */
export async function looksSeveral(ctx, sandbox, prompt) {
  if (prompt.split(/\s+/).filter(Boolean).length < SPLIT_MIN_WORDS) return false;
  try {
    const verdict = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/intake/check`, { prompt }, { timeoutMs: CHECK_WAIT_MS });
    return Boolean(verdict && verdict.verdict === 'several');
  } catch {
    return false;
  }
}

/** The first line of a proposed task's prompt, shortened: what the row previews. */
function preview(prompt) {
  const line = String(prompt || '').split('\n').map((l) => l.replace(/^\s*(\d+[.)]|[-*•])\s+/, '').trim()).find(Boolean) || '';
  return line.length > 140 ? `${line.slice(0, 139)}…` : line;
}

/**
 * The composer's split card (F 2026-10-10 layout): "Split into N tasks?" with one compact row per
 * proposed task — number, editable title, a one-line preview and the prompt collapsed — then In
 * parallel / As a chain and one primary action, Create N tasks, beside Keep as one task. While
 * Claude prepares the proposal the rows shimmer and Keep as one task already works; without a
 * usable proposal the card calls `keep()` by itself (one task, as asked).
 * `create(list, chain)` creates the tasks; `keep()` the original one; `close()` withdraws.
 */
export function splitCard(ctx, sandbox, prompt, { create, keep, close }) {
  const controller = new AbortController();
  const el = h('section', { class: 'split-card', 'aria-label': 'Split proposal', role: 'region' });
  let done = false;
  const finish = (fn) => {
    if (done) return;
    done = true;
    controller.abort();
    fn();
  };
  const keepButton = h('button', { type: 'button', class: 'btn btn-ghost', onClick: () => finish(keep) }, 'Keep as one task');
  const dismiss = iconButton('x', 'Close the proposal', () => finish(close), { class: 'split-x' });
  const head = (title, text) => h('div', { class: 'split-head' }, icon('workflow', { size: 16 }),
    h('div', { class: 'split-heading' }, h('strong', null, title), h('span', { class: 'muted' }, text)), dismiss);
  setChildren(el,
    head('Splitting your request…', 'It looks like separate workstreams. Claude is grouping them.'),
    skeleton('Preparing a split proposal…', 3),
    h('div', { class: 'split-actions' }, h('span', { class: 'split-spacer' }), keepButton));

  (async () => {
    let proposal = null;
    try {
      proposal = await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/intake/proposal`, { prompt }, { timeoutMs: PROPOSAL_WAIT_MS, signal: controller.signal });
    } catch (error) {
      if (error && error.code === 'aborted') return;
      proposal = null;
    }
    if (done) return;
    const tasks = proposal && Array.isArray(proposal.tasks) ? proposal.tasks : [];
    if (tasks.length < 2) {
      toast('Created as one task.', { kind: 'info' });
      finish(keep);
      return;
    }
    const titles = tasks.map((task, i) => h('input', { class: 'split-title', type: 'text', value: task.title, maxlength: 120, 'aria-label': `Title of task ${i + 1}` }));
    const chainFirst = tasks.some((t) => t.after_previous);
    const name = `split-run-${Math.random().toString(36).slice(2, 8)}`;
    const radio = (value, label, checked) => h('label', null, h('input', { type: 'radio', name, value, checked }), label);
    const run = h('div', { class: 'seg split-run', role: 'radiogroup', 'aria-label': 'Run the tasks' },
      radio('parallel', 'In parallel', !chainFirst), radio('chain', 'As a chain', chainFirst));
    const chained = () => Boolean(run.querySelector('input[value="chain"]:checked'));
    const createButton = h('button', { type: 'button', class: 'btn btn-primary', onClick: () => finish(() => create(edited(), chained())) }, `Create ${tasks.length} tasks`);
    function edited() {
      return tasks.map((task, i) => ({ ...task, title: titles[i].value.trim() || task.title }));
    }
    setChildren(el,
      head(`Split into ${tasks.length} tasks?`, chainFirst ? 'Separate workstreams; some must run after another, so a chain fits.' : 'Separate workstreams that could run in parallel.'),
      h('ol', { class: 'split-list' }, tasks.map((task, i) => h('li', { class: 'split-item' },
        h('span', { class: 'split-num', 'aria-hidden': 'true' }, String(i + 1)),
        h('div', { class: 'split-body' },
          h('div', { class: 'split-line' }, titles[i], task.after_previous ? h('span', { class: 'chip xs' }, 'after the previous') : null),
          h('details', { class: 'split-prompt' },
            h('summary', null, h('span', { class: 'split-preview' }, preview(task.prompt)),
              h('span', { class: 'split-more split-more-closed' }, 'Show prompt'), h('span', { class: 'split-more split-more-open' }, 'Hide prompt')),
            h('div', { class: 'split-prompt-text' }, task.prompt)))))),
      h('div', { class: 'split-actions' }, run, h('span', { class: 'split-spacer' }), keepButton, createButton));
    createButton.focus();
  })();

  return { el, cancel: () => finish(() => {}) };
}

/**
 * The composer's calm "working" line while intake decides (F 2026-10-10): a label that follows the
 * stage — reading the request, choosing mode and workflow, starting the task — over a shimmer.
 * Returns {el, stage(text)}.
 */
export function intakeStatus(text) {
  const label = h('span', { class: 'intake-label' }, text);
  const el = h('div', { class: 'intake-status', role: 'status' }, h('span', { class: 'intake-dot', 'aria-hidden': 'true' }), label);
  return {
    el,
    stage(next) {
      if (label.textContent === next) return;
      label.textContent = next;
      label.classList.remove('intake-label-in');
      void label.offsetWidth; // restart the fade for the new stage
      label.classList.add('intake-label-in');
    },
  };
}

const HELD = {
  new_scope: { title: 'New work outside this task', text: 'Held before it reached the agent. Keeping tasks to their scope keeps them predictable.' },
  drops_scope: { title: 'This rules out part of the task', text: 'Held before it reached the agent. Pick the steps it rules out: they are marked skipped and the new direction runs after this task.' },
  redirect: { title: 'This looks like a different task', text: 'Held before it reached the agent. A different task is cleaner as a new one.' },
};

/**
 * The card of a held prompt (TSK-013) with its choices: new scope — Start in parallel, Run after
 * this task; drops scope — skip the chosen steps and run the new direction after; redirect —
 * cancel, revert and start a new task (confirmed); always Send anyway (while running) and close.
 */
export function heldPromptCard(ctx, sandbox, task, prompt, detail) {
  const access = (ctx.store.sandboxById(sandbox.id) || sandbox).access;
  const write = allows(access, 'write');
  const copy = HELD[prompt.class] || HELD.new_scope;
  const slug = encodeURIComponent((ctx.store.sandboxById(sandbox.id) || sandbox).slug);
  let busy = false;

  async function decide(action, extra = {}) {
    if (busy) return;
    busy = true;
    card.classList.add('busy');
    try {
      const outcome = await ctx.api.post(`/api/tasks/${enc(task.id)}/held-prompts/${enc(prompt.id)}`, { action, ...extra }, { timeoutMs: 60000 });
      if (outcome && outcome.task) ctx.store.upsertTask(outcome.task);
      if (outcome && outcome.created) ctx.store.upsertTask(outcome.created);
      const created = outcome && outcome.created;
      toast(outcome ? outcome.note : 'Done.', {
        kind: 'ok',
        action: created ? { label: `Open #${created.number}`, run: () => ctx.navigate(`/s/${slug}/t/${created.number}`) } : undefined,
      });
    } catch (error) {
      errorToast(error, 'Couldn’t handle the held prompt');
    } finally {
      busy = false;
      card.classList.remove('busy');
    }
  }

  const steps = detail && Array.isArray(detail.steps_list)
    ? [...detail.steps_list].sort((a, b) => a.ordinal - b.ordinal).filter((s) => s.status !== 'done' && s.status !== 'skipped')
    : [];
  const checks = steps.map((step) => h('input', { type: 'checkbox', value: step.id, id: `held-step-${step.id}` }));

  const actions = [];
  if (write && prompt.class === 'new_scope') {
    actions.push(h('button', { type: 'button', class: 'btn btn-sm btn-primary', onClick: () => decide('parallel') }, 'Start in parallel'));
    actions.push(h('button', { type: 'button', class: 'btn btn-sm', onClick: () => decide('after') }, 'Run after this task'));
  }
  if (write && prompt.class === 'drops_scope') {
    actions.push(h('button', { type: 'button', class: 'btn btn-sm btn-primary', onClick: () => decide('skip_after', { skip_step_ids: checks.filter((c) => c.checked).map((c) => c.value) }) },
      'Skip the selected steps and run the new direction after this task'));
  }
  if (write && prompt.class === 'redirect') {
    actions.push(h('button', { type: 'button', class: 'btn btn-sm btn-danger', onClick: async () => {
      const ok = await confirmDialog({
        title: `Cancel and revert #${task.number}?`,
        message: `This stops #${task.number}, resets its branch to where it started and archives the task, then starts a new task with your prompt. Its work, committed or not, is kept on a new branch named yulius/reverted/${task.number}-…; nothing is deleted.`,
        confirmLabel: 'Cancel, revert and start the new task',
        danger: true,
      });
      if (ok) decide('revert', { confirm: true });
    } }, 'Cancel, revert and start a new task…'));
  }
  if (write && task.status === 'running') {
    actions.push(h('button', { type: 'button', class: 'btn btn-sm', title: 'Deliver it to the agent unchanged; the override is recorded', onClick: () => decide('send') }, 'Send anyway'));
  }

  const card = h('section', { class: ['held-card', `held-${prompt.class}`], role: 'region', 'aria-label': 'Held prompt' },
    h('div', { class: 'held-head' }, icon('st-attention', { size: 16 }), h('strong', null, copy.title),
      write ? iconButton('x', 'Dismiss (the prompt stays in Claude Code’s input box)', () => decide('dismiss'), { class: 'held-x' }) : null),
    h('blockquote', { class: 'held-prompt' }, prompt.excerpt),
    h('p', { class: 'muted xs held-text' }, copy.text, prompt.typed_by ? ` Typed by ${prompt.typed_by}.` : ''),
    prompt.class === 'drops_scope' && steps.length
      ? h('fieldset', { class: 'held-steps' }, h('legend', { class: 'xs' }, 'Steps it rules out'),
        steps.map((step, i) => h('label', { class: 'check', for: `held-step-${step.id}` }, checks[i], h('span', null, step.title))))
      : null,
    actions.length ? h('div', { class: 'held-actions' }, actions) : h('p', { class: 'muted xs' }, 'Requires write access to decide.'));
  return card;
}
