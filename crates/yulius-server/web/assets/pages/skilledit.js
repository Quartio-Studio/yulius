// Skills → one skill (SPEC-001 UI-033): its SKILL.md in the block editor (mdeditor.js) — formatted as
// you write, frontmatter as properties, the Markdown source on demand — with Save (⌘S) and a guard
// against leaving unsaved changes. Repository skills save into the repository; instance skills need
// an instance administrator; the Open Mercato pack is read-only. Saving sends
// PUT /api/sandboxes/{sid}/skills/{name} {"content"} (docs/api.md, proposed).

import { enc } from '../api.js';
import { h, icon, setChildren, placeholder, loadError, iconBadge, confirmDialog, toast } from '../dom.js';
import { href, navigate } from '../router.js';
import { allows } from '../status.js';
import { relLong } from '../format.js';
import { createMarkdownEditor } from '../mdeditor.js';
import { SOURCE_BADGE } from './skills.js';

/** Mount the editor of one skill. */
export function mountSkillEditor(ctx, sandbox, name) {
  const base = `/s/${encodeURIComponent(sandbox.slug)}/skills`;
  let skill = null;
  let editor = null;
  let saved = ''; // the Markdown as last saved (in the editor's own form)
  let dirty = false;
  let savedAt = null;
  let alive = true;
  const reads = new AbortController();

  const status = h('span', { class: 'se-status', role: 'status' });
  const save = h('button', { type: 'button', class: 'btn btn-primary', disabled: true, onClick: () => doSave() }, 'Save');
  const discard = h('button', { type: 'button', class: 'btn', hidden: true, onClick: () => { editor.setMarkdown(saved); saved = editor.getMarkdown(); setDirty(false); } }, 'Discard');
  const sourceToggle = h('button', { type: 'button', class: 'btn-icon se-source', 'aria-pressed': 'false', 'aria-label': 'Show the Markdown source', title: 'Markdown source', hidden: true,
    onClick: () => {
      const on = !editor.sourceMode;
      editor.setSource(on);
      sourceToggle.setAttribute('aria-pressed', String(on));
    } }, icon('code', { size: 16 }));
  const badges = h('span', { class: 'se-badges' });
  const pathEl = h('div', { class: 'muted small mono se-path' });
  const note = h('div', { class: 'se-note' });
  const head = h('header', { class: 'gt-head se-head' },
    h('a', { class: 'gt-back', href: href(base), dataset: { route: base } }, icon('back', { size: 14 }), 'Skills'),
    h('div', { class: 'gt-title-row' }, h('h1', { class: 'page-title se-title' }, name), badges, h('span', { class: 'spacer' }), status, sourceToggle, discard, save),
    pathEl, note);
  const body = h('div', { class: 'se-body' }, placeholder('Loading the skill…', 'loading'));
  const el = h('div', { class: 'page se-page' }, h('div', { class: 'page-inner se-inner' }, head, body));

  function setDirty(next) {
    dirty = next;
    save.disabled = !dirty;
    discard.hidden = !dirty;
    status.textContent = dirty ? 'Unsaved changes' : savedAt ? `Saved ${relLong(savedAt)}` : '';
    status.classList.toggle('se-dirty', dirty);
  }

  let changeFrame = 0;
  const onChange = () => {
    if (changeFrame) return;
    changeFrame = requestAnimationFrame(() => { changeFrame = 0; if (editor) setDirty(editor.getMarkdown() !== saved); });
  };

  async function doSave() {
    if (!editor || !dirty || save.classList.contains('busy')) return;
    const content = editor.getMarkdown();
    save.classList.add('busy');
    save.disabled = true;
    try {
      await ctx.api.put(`/api/sandboxes/${enc(sandbox.id)}/skills/${enc(name)}`, { content });
      if (!alive) return;
      saved = content;
      savedAt = new Date().toISOString();
      setDirty(editor.getMarkdown() !== saved);
      toast(`Saved ${name}`, { kind: 'ok' });
    } catch (error) {
      if (!alive) return;
      save.disabled = false;
      const missing = error.status === 422 || error.status === 404 || error.status === 405;
      status.textContent = missing && !/read-only|administrators/.test(error.message)
        ? `Couldn’t save: ${error.message}. Saving a skill's text needs the server's skill content API (docs/api.md).`
        : `Couldn’t save: ${error.message}`;
      status.classList.add('se-error');
    } finally {
      save.classList.remove('busy');
    }
  }

  // ⌘S / Ctrl+S saves.
  el.addEventListener('keydown', (event) => {
    if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === 's') { event.preventDefault(); doSave(); }
  });

  // Unsaved changes: the browser asks before the tab closes; links inside the app ask here.
  const onBeforeUnload = (event) => { if (dirty) { event.preventDefault(); event.returnValue = ''; } };
  const onLink = async (event) => {
    const link = event.target.closest && event.target.closest('a[data-route]');
    if (!dirty || !link || event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const ok = await confirmDialog({ title: 'Leave without saving?', message: `Your changes to ${name} are not saved.`, confirmLabel: 'Leave', danger: true });
    if (!ok) return;
    setDirty(false);
    navigate(link.dataset.route);
  };
  window.addEventListener('beforeunload', onBeforeUnload);
  document.addEventListener('click', onLink, true);

  async function load() {
    try {
      const [list, data] = await Promise.all([
        ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/skills`, { signal: reads.signal }),
        ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/skills/${enc(name)}`, { signal: reads.signal }),
      ]);
      if (!alive) return;
      skill = (Array.isArray(list) ? list : []).find((s) => s.name === name) || { name, source: 'repository' };
      const write = allows(sandbox.access, 'write');
      const why = skill.source === 'open-mercato' ? 'The Open Mercato pack is read-only: it is shared by every instance.'
        : !write ? 'Read-only: editing needs write access to this sandbox.'
          : skill.source === 'instance' && !ctx.isInstanceAdmin() ? 'Read-only: instance skills are edited by instance administrators.' : null;
      const [badgeIcon, badgeLabel, badgeTone] = SOURCE_BADGE[skill.source] || ['book', skill.source, ''];
      setChildren(badges, iconBadge(badgeIcon, badgeLabel, badgeTone), skill.enabled === false ? iconBadge('minimize', 'Off', '') : null);
      pathEl.textContent = skill.path || '';
      setChildren(note, why ? h('p', { class: 'banner small' }, icon('lock', { size: 14 }), h('span', { class: 'banner-text' }, why)) : null);
      editor = createMarkdownEditor({ markdown: (data && data.content) || '', readOnly: Boolean(why), onChange });
      saved = editor.getMarkdown();
      sourceToggle.hidden = false;
      save.hidden = Boolean(why);
      setChildren(body, editor.el);
      setDirty(false);
    } catch (error) {
      if (!alive || error.code === 'aborted') return;
      setChildren(body, loadError(`Couldn’t load ${name}: ${error.message}`, () => { setChildren(body, placeholder('Loading the skill…', 'loading')); load(); }));
    }
  }
  load();

  return {
    el,
    start() { if (editor) editor.focus(); },
    destroy() {
      alive = false;
      reads.abort();
      cancelAnimationFrame(changeFrame);
      window.removeEventListener('beforeunload', onBeforeUnload);
      document.removeEventListener('click', onLink, true);
      if (editor) editor.destroy();
    },
  };
}
