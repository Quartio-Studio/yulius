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
    return h('li', null, `${notice.test} failed at ${notice.revision.slice(0, 12)} (${origin})`);
  });
  return [h('div', { class: 'banner banner-warn', role: 'status' },
    icon('warn', { size: 16 }),
    h('div', { class: 'banner-text' },
      h('strong', null, sandbox.name || sandbox.slug),
      h('ul', null, failures),
      'No task is working on the repair. ',
      h('a', { href: href(path), dataset: { route: path, focusKey: path } }, canRun ? 'Review and run again' : 'Review the run'),
      canRun ? [' · ', h('a', { href: href(project), dataset: { route: project, focusKey: project } }, 'Start a task')] : null))];
}
