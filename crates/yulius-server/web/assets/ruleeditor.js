// The start-rule editor of the composer (SPEC-022): a person picks conditions, each a task of the
// sandbox and a state, and whether all or any must hold. It writes the sentence the server
// parses (SPEC-017), so nobody has to type `#12 done and #13 done`.

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
