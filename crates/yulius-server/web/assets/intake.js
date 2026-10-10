// Task intake in the UI (SPEC-001 TSK-011..013, SPEC-002 ADR-16): the composer's split check
// and card, the "Run after" choice, and the card of a prompt held by the scope check.
// Every API path is a literal at its call site (tests/ui_contract.rs checks them).

import { enc } from './api.js';
import { h, icon, iconButton, setChildren, toast, errorToast, confirmDialog } from './dom.js';
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
    .filter((t) => !t.archived && (isLive(t.status) || t.status === 'chained'))
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

/**
 * The composer's split card: "This looks like N separate tasks", the proposed tasks with
 * editable titles, and Create N tasks (in parallel) · Create as a chain · Keep as one task.
 * `create(list, chain)` creates the tasks; `keep()` creates the original one. While the
 * proposal is prepared, Keep as one task already works; without a usable proposal the card
 * calls `keep()` by itself (one task, as asked).
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
  const keepButton = h('button', { type: 'button', class: 'btn', onClick: () => finish(keep) }, 'Keep as one task');
  const dismiss = iconButton('x', 'Close the proposal', () => finish(close), { class: 'split-x' });
  setChildren(el,
    h('div', { class: 'split-head' }, icon('workflow', { size: 16 }),
      h('strong', null, 'This looks like separate tasks.'), h('span', { class: 'muted' }, 'Preparing a proposal…'), dismiss),
    h('div', { class: 'split-actions' }, keepButton));

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
    const titles = tasks.map((task, i) => h('input', { class: 'input split-title', type: 'text', value: task.title, maxlength: 120, 'aria-label': `Title of task ${i + 1}` }));
    const chainFirst = tasks.some((t) => t.after_previous);
    const parallel = h('button', { type: 'button', class: ['btn', !chainFirst && 'btn-primary'], onClick: () => finish(() => create(edited(), false)) }, `Create ${tasks.length} tasks (in parallel)`);
    const chain = h('button', { type: 'button', class: ['btn', chainFirst && 'btn-primary'], onClick: () => finish(() => create(edited(), true)) }, 'Create as a chain');
    function edited() {
      return tasks.map((task, i) => ({ ...task, title: titles[i].value.trim() || task.title }));
    }
    setChildren(el,
      h('div', { class: 'split-head' }, icon('workflow', { size: 16 }),
        h('strong', null, `This looks like ${tasks.length} separate tasks.`),
        h('span', { class: 'muted' }, chainFirst ? 'Some must run after another: a chain fits.' : 'They could run in parallel.'), dismiss),
      h('ol', { class: 'split-list' }, tasks.map((task, i) => h('li', { class: 'split-item' },
        titles[i],
        task.after_previous ? h('span', { class: 'chip xs' }, 'after the previous') : null,
        h('details', { class: 'split-prompt' }, h('summary', null, 'Prompt'), h('pre', null, task.prompt))))),
      h('div', { class: 'split-actions' }, parallel, chain, keepButton));
    titles[0].focus();
  })();

  return { el, cancel: () => finish(() => {}) };
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
      errorToast(error, 'Could not handle the held prompt');
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
