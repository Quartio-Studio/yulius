// Skills → New skill (SPEC-001 UI-035): name it, say when Claude Code should use it, pick a
// template and write the rest in the block editor (mdeditor.js). The name becomes the folder
// (.agents/skills/{name}/SKILL.md in the sandbox repository, where the skill catalog finds it); the
// frontmatter is written for you. Creating uses the built-in editor's files API (root `sandbox`):
// the folder, then the file (refused when it exists, so nothing is overwritten).

import { enc } from '../api.js';
import { h, icon, setChildren, confirmDialog, toast, invalid } from '../dom.js';
import { href, navigate } from '../router.js';
import { allows } from '../status.js';
import { createMarkdownEditor } from '../mdeditor.js';

const TEMPLATES = [
  { id: 'checklist', name: 'Checklist', icon: 'check', use: 'Use when …', body: '## Steps\n\n1. \n2. \n3. \n\n## Done when\n\n- [ ] \n' },
  { id: 'conventions', name: 'Conventions', icon: 'code', use: 'Use when writing or changing …', body: '## Do\n\n- \n\n## Don’t\n\n- \n\n## Example\n\n```\n\n```\n' },
  { id: 'review', name: 'Review', icon: 'search', use: 'Use when reviewing a change before it is delivered.', body: '## Check\n\n- [ ] The change does what the task asked\n- [ ] Tests cover the new behaviour\n- [ ] No secrets or credentials in the diff\n- [ ] Docs are updated\n' },
  { id: 'voice', name: 'Voice and tone', icon: 'mail', use: 'Use when writing copy users will read.', body: '## Voice\n\n- \n\n## Words we use\n\n- \n\n## Words we avoid\n\n- \n' },
  { id: 'blank', name: 'Blank', icon: 'plus', use: '', body: '' },
];

/** A skill name as Claude Code expects it: lowercase letters, digits and hyphens. */
export function skillSlug(text) {
  return String(text || '').toLowerCase().normalize('NFKD').replace(/[̀-ͯ]/g, '').replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 64);
}

/** Mount the New skill page. */
export function mountSkillCreator(ctx, sandbox) {
  const base = `/s/${encodeURIComponent(sandbox.slug)}/skills`;
  const write = allows(sandbox.access, 'write');
  let template = TEMPLATES[0];
  let usePlaceholder = template.use;
  let pristine = '';
  let created = false;

  const nameInput = h('input', { class: 'wf-name', placeholder: 'Name your skill', 'aria-label': 'Skill name', maxlength: 64, autocomplete: 'off', onInput: () => update() });
  const useInput = h('input', { class: 'wf-desc', placeholder: usePlaceholder, 'aria-label': 'When Claude Code should use it', maxlength: 300, onInput: () => update() });
  const pathEl = h('div', { class: 'muted small mono se-path' });
  const create = h('button', { type: 'button', class: 'btn btn-primary', disabled: !write, onClick: () => doCreate() }, 'Create skill');
  const templates = h('div', { class: 'sn-templates', role: 'radiogroup', 'aria-label': 'Template' });
  const editor = createMarkdownEditor({ markdown: template.body, readOnly: !write });
  pristine = editor.getMarkdown();

  const head = h('header', { class: 'gt-head se-head' },
    h('a', { class: 'gt-back', href: href(base), dataset: { route: base } }, icon('back', { size: 14 }), 'Skills'),
    h('div', { class: 'gt-title-row' }, h('div', { class: 'wf-titles' }, nameInput, useInput), create),
    pathEl,
    !write ? h('p', { class: 'banner small' }, icon('lock', { size: 14 }), h('span', { class: 'banner-text' }, 'Creating skills needs write access to this sandbox.')) : null);
  const setup = h('div', { class: 'sn-setup' },
    h('div', { class: 'sn-row' }, h('span', { class: 'wf-label' }, 'Start from'), templates));
  const el = h('div', { class: 'page se-page' }, h('div', { class: 'page-inner se-inner' }, h('h1', { class: 'sr-only' }, 'New skill'), head, setup, h('div', { class: 'se-body' }, editor.el)));

  function update() {
    const slug = skillSlug(nameInput.value);
    const file = `.agents/skills/${slug || '…'}/SKILL.md`;
    setChildren(pathEl, slug && slug !== nameInput.value.trim() ? [h('span', null, 'Saved as '), h('strong', null, slug), ' · '] : null, file);
    create.disabled = !write;
  }

  function renderTemplates() {
    setChildren(templates, TEMPLATES.map((t) => h('button', {
      type: 'button', role: 'radio', 'aria-checked': String(t === template), class: ['wf-chip', 'sn-template', t === template && 'on'],
      onClick: () => pickTemplate(t),
    }, icon(t.icon, { size: 14 }), t.name)));
  }

  async function pickTemplate(t) {
    if (t === template) return;
    if (editor.getMarkdown() !== pristine && !(await confirmDialog({ title: `Start from ${t.name}?`, message: 'What you wrote below is replaced by the template.', confirmLabel: 'Replace', danger: true }))) return;
    template = t;
    editor.setMarkdown(t.body);
    pristine = editor.getMarkdown();
    usePlaceholder = t.use || 'When should Claude Code use it?';
    useInput.placeholder = usePlaceholder;
    renderTemplates();
  }

  function content() {
    const slug = skillSlug(nameInput.value);
    const use = useInput.value.trim().replace(/\s+/g, ' ');
    const quoted = /[:#'"]|^[-?!&*[\]{}|>%@`]/.test(use) ? JSON.stringify(use) : use;
    const body = editor.getMarkdown().replace(/^---\n[\s\S]*?\n---\n*/, '').trim();
    return `---\nname: ${slug}\ndescription: ${quoted}\n---\n\n${body}\n`;
  }

  async function doCreate() {
    if (!write || create.classList.contains('busy')) return;
    const slug = skillSlug(nameInput.value);
    if (!slug) { invalid(nameInput, 'Give the skill a name.'); return; }
    if (slug === 'new') { invalid(nameInput, '“new” is reserved: pick another name.'); return; }
    if (!useInput.value.trim()) { invalid(useInput, 'Say when Claude Code should use it: that is how it picks the skill.'); return; }
    create.classList.add('busy');
    const files = `/api/sandboxes/${enc(sandbox.id)}/files`;
    try {
      const existing = await ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/skills`).catch(() => []);
      if ((Array.isArray(existing) ? existing : []).some((sk) => sk.name === slug)) { invalid(nameInput, `A skill named ${slug} already exists.`); return; }
      // The folders may exist already (.agents/skills, or the skill's own): that is fine.
      for (const dir of ['.agents', '.agents/skills', `.agents/skills/${slug}`]) {
        await ctx.api.post(files, { root: 'sandbox', path: dir, kind: 'dir' }).catch((error) => { if (error.status !== 409) throw error; });
      }
      await ctx.api.put(`${files}/content`, { root: 'sandbox', path: `.agents/skills/${slug}/SKILL.md`, content: content() });
      created = true;
      toast(`Created ${slug}`, { kind: 'ok' });
      navigate(`${base}/${encodeURIComponent(slug)}`);
    } catch (error) {
      if (error.status === 409) invalid(nameInput, `A skill named ${slug} already exists.`);
      else toast(`Couldn’t create: ${error.message}`, { kind: 'error' });
    } finally {
      create.classList.remove('busy');
    }
  }

  const dirty = () => !created && (nameInput.value.trim() || useInput.value.trim() || editor.getMarkdown() !== pristine);
  const onBeforeUnload = (event) => { if (dirty()) { event.preventDefault(); event.returnValue = ''; } };
  const onLink = async (event) => {
    const link = event.target.closest && event.target.closest('a[data-route]');
    if (!link || !dirty() || event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    if (await confirmDialog({ title: 'Leave without creating?', message: 'The new skill is not saved.', confirmLabel: 'Leave', danger: true })) { created = true; navigate(link.dataset.route); }
  };
  window.addEventListener('beforeunload', onBeforeUnload);
  document.addEventListener('click', onLink, true);
  el.addEventListener('keydown', (event) => {
    if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === 's') { event.preventDefault(); doCreate(); }
  });

  renderTemplates();
  update();

  return {
    el,
    start() { nameInput.focus(); },
    destroy() { window.removeEventListener('beforeunload', onBeforeUnload); document.removeEventListener('click', onLink, true); editor.destroy(); },
  };
}
