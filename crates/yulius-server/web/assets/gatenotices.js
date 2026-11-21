// A project failure whose task cannot receive the outcome. Writers can rerun from the run view.
import { h, icon } from './dom.js';
import { href } from './router.js';

export function gateNotices(sandbox, notices, canRun) {
  return (notices || []).map((notice) => {
    const path = `/s/${encodeURIComponent(sandbox.slug)}/ci?run=${encodeURIComponent(notice.run_id)}`;
    return h('div', { class: 'banner banner-warn', role: 'status' },
      icon('warn', { size: 16 }),
      h('span', { class: 'banner-text' }, `${sandbox.name || sandbox.slug}: ${notice.test} failed at ${notice.revision.slice(0, 12)}. Its task is unavailable. `,
        h('a', { href: href(path), dataset: { route: path } }, canRun ? 'Review and run again' : 'Review the run')));
  });
}
