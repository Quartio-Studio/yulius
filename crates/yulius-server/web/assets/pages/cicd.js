// CI/CD page (SPEC-151, ADR-0125): CI (tests) and CD (deployment) as two sections of one page,
// chosen with the Secrets page's tabs. The section is kept in `?section=cd` (CI is the plain
// address, the one the `yulius/ci` commit status links to); `/s/<slug>/cd` opens the CD tab too.

import { sectionedPage } from './common.js';
import { mountCiSection, CI_SUB } from './ci.js';
import { mountCdSection, CD_SUB } from './cd.js';

const SECTIONS = [
  { value: '', label: 'CI (tests)', icon: 'check', sub: CI_SUB, mount: mountCiSection },
  { value: 'cd', label: 'CD (deployment)', icon: 'upload', sub: CD_SUB, mount: mountCdSection },
];

/** Mount the CI/CD page: `/s/<slug>/ci`, `?section=cd` for the CD tab. */
export function mountCiCdPage(ctx, sandbox, route) {
  const path = `/s/${encodeURIComponent(sandbox.slug)}/ci`;
  const page = sectionedPage(ctx, sandbox, route, { title: 'CI/CD', path, sections: SECTIONS });
  return { ...page, key: `ci:${sandbox.id}` };
}
