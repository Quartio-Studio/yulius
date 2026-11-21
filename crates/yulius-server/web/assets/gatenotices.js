// Project integration failures with no working repair owner.
import { h, icon } from './dom.js';
import { href } from './router.js';

export function gateNotices(sandbox, notices, canRun) {
  const project = `/s/${encodeURIComponent(sandbox.slug)}`;
  return (notices || []).map((notice) => {
    const path = `${project}/ci?run=${encodeURIComponent(notice.run_id)}`;
    const origin = notice.merge_task ? ` (task #${notice.merge_task})` : ' (untracked merge)';
    return h('div', { class: 'banner banner-warn', role: 'status' },
      icon('warn', { size: 16 }),
      h('span', { class: 'banner-text' }, `${sandbox.name || sandbox.slug}: ${notice.test} failed at ${notice.revision.slice(0, 12)}${origin}. No task is working on the repair. `,
        h('a', { href: href(path), dataset: { route: path } }, canRun ? 'Review and run again' : 'Review the run'),
        canRun ? [' · ', h('a', { href: href(project), dataset: { route: project } }, 'Start a task')] : null));
  });
}
