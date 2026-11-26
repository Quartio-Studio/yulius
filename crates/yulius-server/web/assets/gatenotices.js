// Project integration failures with no working repair owner.
import { h, icon } from './dom.js';
import { href } from './router.js';

export function gateNotices(sandbox, notices, canRun) {
  if (!notices?.length) return [];
  const project = `/s/${encodeURIComponent(sandbox.slug)}`;
  // The project gate reports each origin separately and one latest run for retrying them.
  const path = `${project}/ci?run=${encodeURIComponent(notices[0].run_id)}`;
  const failures = notices.map((notice) => {
    const origin = notice.merge_task ? `task #${notice.merge_task}` : 'untracked merge';
    return h('li', null, `${notice.test} · first seen at ${notice.revision.slice(0, 12)} (${origin})`);
  });
  return [h('div', { class: 'banner banner-warn', role: 'status' },
    icon('warn', { size: 16 }),
    h('div', { class: 'banner-text' },
      h('strong', null, `Unresolved integration gate for ${sandbox.name || sandbox.slug}`),
      h('ul', null, failures),
      'No task is working on the repair. ',
      h('a', { href: href(path), dataset: { route: path, focusKey: `${project}/ci#gate-run` } }, canRun ? 'Review and run again' : 'Review the run'),
      canRun ? [' · ', h('a', { href: href(project), dataset: { route: project, focusKey: project } }, 'Start a task')] : null))];
}

/**
 * The merge queue stopped by a red after-merge gate (SPEC-676 RGS-010): which failures hold it,
 * which armed merges wait, and the run to read. Nothing while merges go on.
 */
export function mergeStopBanner(sandbox, stop, canRun) {
  if (!stop?.failures?.length) return [];
  const project = `/s/${encodeURIComponent(sandbox.slug)}`;
  const path = `${project}/ci?run=${encodeURIComponent(stop.run_id)}`;
  const failures = stop.failures.map((failure) => {
    const origin = failure.merge_task ? `task #${failure.merge_task}` : 'no task named yet';
    return h('li', null, `${failure.test} · first seen at ${failure.revision.slice(0, 12)} (${origin})`);
  });
  const waiting = stop.waiting || [];
  const held = waiting.map((w) => (w.pull_request ? `#${w.pull_request} (task #${w.task})` : `task #${w.task}`)).join(', ');
  const count = waiting.length === 1 ? '1 armed merge waits' : `${waiting.length} armed merges wait`;
  return [h('div', { class: 'banner banner-warn', role: 'status', 'aria-label': 'Merges stopped' },
    icon('warn', { size: 16 }),
    h('div', { class: 'banner-text' },
      h('strong', null, 'Merges are stopped: the after-merge gate of the integration branch is red'),
      h('ul', null, failures),
      `Only its repair merges. ${waiting.length ? `${count}: ${held}. ` : ''}They merge by themselves once a later revision's gate is green. `,
      h('a', { href: href(path), dataset: { route: path, focusKey: `${project}/ci#stop-run` } }, canRun ? 'Review and run again' : 'Review the run')))];
}
