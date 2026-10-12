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
 * Opens the editor. `tasks` are the sandbox's tasks (see ruleChoices), `value` the rule picked
 * before (`{ join, conditions }`) or null; `onUse(rule)` gets the rule when Use this rule is
 * pressed. The first condition names the newest task, each added one a task not yet named.
 */
export function openRuleEditor({ tasks, value, onUse }) {
  const choices = ruleChoices(tasks);
  if (!choices.length) return;
  const known = new Set(choices.map((t) => t.number));
  const rule = {
    join: value && value.join === 'or' ? 'or' : 'and',
    conditions: value ? value.conditions.filter((c) => known.has(c.number)).map((c) => ({ ...c })) : [],
  };
  if (!rule.conditions.length) rule.conditions.push({ number: choices[0].number, state: 'done' });

  const name = `rule-join-${Math.random().toString(36).slice(2, 8)}`;
  const radio = (join, label) => h('label', null,
    h('input', { type: 'radio', name, value: join, checked: rule.join === join, onChange: () => { rule.join = join; showPreview(); } }), label);
  const joinGroup = h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Start when' }, radio('and', 'All are true'), radio('or', 'Any is true'));
  const list = h('ol', { class: 'rule-conditions' });
  const preview = h('p', { class: 'rule-preview', role: 'status' });
  const addButton = h('button', { type: 'button', class: 'btn btn-sm', onClick: add }, icon('plus', { size: 14 }), 'Add condition');

  function add() {
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
    const task = h('select', { class: 'select select-sm rule-task', 'aria-label': `Task of condition ${n}`, onChange: (event) => { condition.number = Number(event.target.value); showPreview(); } },
      choices.map((t) => h('option', { value: String(t.number), selected: t.number === condition.number }, `#${t.number} ${t.title}`)));
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
    showPreview();
  }

  render();
  openModal({
    title: 'Start when a rule is true',
    className: 'rule-editor',
    body: h('div', { class: 'stack' }, joinGroup, list, h('div', null, addButton), preview,
      h('p', { class: 'field-hint' }, 'Has ended means the task stopped, failed, was closed or was deleted.')),
    actions: [
      { label: 'Cancel', value: false },
      { label: 'Use this rule', kind: 'primary', run: () => onUse({ join: rule.join, conditions: rule.conditions.map((c) => ({ ...c })) }) },
    ],
  });
  list.querySelector('select').focus();
}
