// Skills → one skill (SPEC-001 UI-033): its SKILL.md in the block editor (mdeditor.js) — formatted as
// you write, frontmatter as properties, the Markdown source on demand — with Save (⌘S) and a guard
// against leaving unsaved changes. A repository skill is a file in the sandbox repository
// (.agents/skills or .claude/skills): it is read and saved with the built-in editor's files API
// (root `sandbox`; the save carries the read's sha256, so a file changed meanwhile is a conflict, not
// an overwrite). Instance skills and the included skills are kept by the server: read-only here.

import { enc } from '../api.js';
import { h, icon, setChildren, placeholder, loadError, iconBadge, confirmDialog, toast } from '../dom.js';
import { href, navigate } from '../router.js';
import { allows } from '../status.js';
import { relLong } from '../format.js';
import { createMarkdownEditor } from '../mdeditor.js';
import { SOURCE_BADGE } from './skills.js';
import { BRAND } from '../brand.js';

/** Mount the editor of one skill. */
export function mountSkillEditor(ctx, sandbox, name) {
  const base = `/s/${encodeURIComponent(sandbox.slug)}/skills`;
  let skill = null;
  let editor = null;
  let saved = ''; // the Markdown as last saved (in the editor's own form)
  let dirty = false;
  let savedAt = null;
  let sha = null; // the repository file's sha256 as read or last saved
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
      const written = await ctx.api.put(`/api/sandboxes/${enc(sandbox.id)}/files/content`, { root: 'sandbox', path: skill.path, content, base_sha256: sha });
      if (!alive) return;
      sha = written.sha256;
      saved = content;
      savedAt = new Date().toISOString();
      setDirty(editor.getMarkdown() !== saved);
      toast(`Saved ${name}`, { kind: 'ok' });
    } catch (error) {
      if (!alive) return;
      save.disabled = false;
      status.textContent = error.status === 409
        ? 'Not saved: this skill changed since you opened it. Copy your text, then reload to see the new version.'
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
      const list = await ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/skills`, { signal: reads.signal });
      if (!alive) return;
      skill = (Array.isArray(list) ? list : []).find((s) => s.name === name) || { name, source: 'repository', path: `.agents/skills/${name}/SKILL.md` };
      // A repository skill is read as a file (its sha256 guards the save) by people who may write; the
      // files API is write-level, so readers and the other sources read it as the catalog gives it.
      const write = allows(sandbox.access, 'write');
      let data;
      if (skill.source === 'repository' && skill.path && write) {
        data = await ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/files/content?${new URLSearchParams({ root: 'sandbox', path: skill.path })}`, { signal: reads.signal });
        sha = data.sha256 || null;
      } else {
        data = await ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/skills/${enc(name)}`, { signal: reads.signal });
      }
      if (!alive) return;
      const why = skill.source !== 'repository' && skill.source !== 'instance' ? `Read-only: included skills ship with ${BRAND.name} and stay current with it.`
        : skill.source !== 'repository' ? 'Read-only: instance skills are kept by the server and are not edited here.'
          : !write ? 'Read-only: editing needs write access to this sandbox.' : null;
      const [badgeIcon, badgeLabel, badgeTone] = SOURCE_BADGE[skill.source] || ['lock', 'Included', ''];
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
