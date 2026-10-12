// The start-rule editor of the composer (SPEC-022): a person picks conditions, each a task of the
// sandbox and a state, and whether all or any must hold. It writes the sentence the server
// parses (SPEC-017), so nobody has to type `#12 done and #13 done`.

import { h, icon, iconButton, openModal, setChildren } from './dom.js';

/** The states a condition can ask for, in the words of the rule language (RUL-001). */
export const RULE_STATES = [
  { value: 'done', label: 'is done' },
  { value: 'merged', label: 'is merged' },
  { value: 'ended', label: 'has ended' },
];

/** Most tasks a condition offers: the newest ones, as the "After #N" choices. */
const MAX_CHOICES = 50;

/** Most conditions in a rule: the server refuses a rule naming more tasks (MAX_ATOMS, RUL-002). */
export const RULE_MAX_CONDITIONS = 16;

/**
 * The canonical rule text for `{ join: 'and' | 'or', conditions: [{ number, state }] }`, as the
 * server stores it: `#12 done and #13 merged`. No conditions give the empty string.
 */
export function ruleText(rule) {
  const join = rule.join === 'or' ? ' or ' : ' and ';
  return rule.conditions.map((c) => `#${c.number} ${c.state}`).join(join);
}

/** The tasks a condition can name: not archived, newest first. */
export function ruleChoices(tasks) {
  return (tasks || [])
    .filter((t) => !t.archived)
    .sort((a, b) => b.number - a.number)
    .slice(0, MAX_CHOICES);
}

/**
 * The options of one condition's task: the offered `choices`, plus the picked task `number` when
 * it is not among them, so reopening never changes a rule unseen. `tasks` are the store's tasks,
 * which leave archived ones out: a task found there is only older than the choices, one not found
 * was archived or removed since it was picked.
 */
export function conditionChoices(choices, number, tasks) {
  const options = choices.map((t) => ({ number: t.number, label: `#${t.number} ${t.title}` }));
  if (choices.some((t) => t.number === number)) return options;
  const task = (tasks || []).find((t) => t.number === number);
  return [...options, { number, label: task ? `#${number} ${task.title}` : `#${number} (archived or removed)` }];
}

/**
 * Opens the editor. `tasks` are the sandbox's tasks (see ruleChoices), `value` the rule picked
 * before (`{ join, conditions }`) or null; `onUse(rule)` gets the rule when Use this rule is
 * pressed. The first condition names the newest task, each added one a task not yet named.
 */
export function openRuleEditor({ tasks, value, onUse }) {
  const choices = ruleChoices(tasks);
  if (!choices.length) return;
  const rule = {
    join: value && value.join === 'or' ? 'or' : 'and',
    conditions: value ? value.conditions.map((c) => ({ ...c })) : [],
  };
  if (!rule.conditions.length) rule.conditions.push({ number: choices[0].number, state: 'done' });

  const name = `rule-join-${Math.random().toString(36).slice(2, 8)}`;
  const radio = (join, label) => h('label', null,
    h('input', { type: 'radio', name, value: join, checked: rule.join === join, onChange: () => { rule.join = join; showPreview(); } }), label);
  const joinGroup = h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Start when' }, radio('and', 'All are true'), radio('or', 'Any is true'));
  const list = h('ol', { class: 'rule-conditions' });
  const preview = h('p', { class: 'rule-preview', role: 'status' });
  const addButton = h('button', { type: 'button', class: 'btn btn-sm', onClick: add }, icon('plus', { size: 14 }), 'Add condition');
  const limitNote = h('span', { class: 'field-hint' }, `A rule names at most ${RULE_MAX_CONDITIONS} conditions.`);

  function add() {
    if (rule.conditions.length >= RULE_MAX_CONDITIONS) return;
    const used = new Set(rule.conditions.map((c) => c.number));
    const next = choices.find((t) => !used.has(t.number)) || choices[0];
    rule.conditions.push({ number: next.number, state: 'done' });
    render();
    list.querySelector('li:last-child select').focus();
  }

  function remove(index) {
    rule.conditions.splice(index, 1);
    render();
    const rows = list.querySelectorAll('li select');
    (rows[Math.min(index, rule.conditions.length - 1) * 2] || addButton).focus();
  }

  function row(condition, index) {
    const n = index + 1;
    // The button with <selectedcontent> lets a long task title end in an ellipsis instead of wrapping.
    const task = h('select', { class: 'select select-sm rule-task', 'aria-label': `Task of condition ${n}`, onChange: (event) => { condition.number = Number(event.target.value); showPreview(); } },
      h('button', null, h('selectedcontent')),
      ...conditionChoices(choices, condition.number, tasks).map((o) => h('option', { value: String(o.number), selected: o.number === condition.number }, o.label)));
    const state = h('select', { class: 'select select-sm rule-state', 'aria-label': `State of condition ${n}`, onChange: (event) => { condition.state = event.target.value; showPreview(); } },
      RULE_STATES.map((s) => h('option', { value: s.value, selected: s.value === condition.state }, s.label)));
    return h('li', { class: 'rule-condition' }, task, state,
      rule.conditions.length > 1 ? iconButton('x', `Remove condition ${n}`, () => remove(index)) : null);
  }

  function showPreview() {
    preview.textContent = `Starts by itself when ${ruleText(rule)}`;
  }

  function render() {
    setChildren(list, rule.conditions.map(row));
    joinGroup.hidden = rule.conditions.length < 2;
    const full = rule.conditions.length >= RULE_MAX_CONDITIONS;
    addButton.disabled = full;
    limitNote.hidden = !full;
    showPreview();
  }

  render();
  openModal({
    title: 'Start when a rule is true',
    className: 'rule-editor',
    body: h('div', { class: 'stack' }, joinGroup, list, h('div', { class: 'rule-add' }, addButton, limitNote), preview,
      h('p', { class: 'field-hint' }, 'Has ended means the task stopped, failed, was closed or was deleted.')),
    actions: [
      { label: 'Cancel', value: false },
      { label: 'Use this rule', kind: 'primary', run: () => onUse({ join: rule.join, conditions: rule.conditions.map((c) => ({ ...c })) }) },
    ],
  });
  list.querySelector('select').focus();
}
