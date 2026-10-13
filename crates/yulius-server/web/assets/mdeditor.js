// A block editor for Markdown, Notion-style (SPEC-001 UI-033): the text shows formatted while you
// write — headings, paragraphs, lists, to-dos, quotes, code, dividers — and reads back as Markdown.
// Frontmatter shows as properties. "/" in an empty block opens the block menu; "# ", "- ", "1. ",
// "[] ", "> " and "```" turn a block into that kind; ⌘B ⌘I ⌘E ⌘K format text; a toolbar appears
// over selected text. Anything it does not model (tables, HTML) stays as a raw Markdown block, so a
// file reads back unchanged where you did not edit it. The file is untrusted: the DOM is built from
// text nodes, never from HTML, and only http(s), mailto and relative links are kept.

import { h, openMenu, openModal } from './dom.js';

const TEXT_TYPES = new Set(['p', 'h1', 'h2', 'h3', 'ul', 'ol', 'todo', 'quote']);
const LIST_TYPES = new Set(['ul', 'ol', 'todo']);
const PLACEHOLDER = { p: 'Type “/” for blocks', h1: 'Heading 1', h2: 'Heading 2', h3: 'Heading 3', ul: 'List', ol: 'List', todo: 'To-do', quote: 'Quote' };
const BLOCK_MENU = [
  ['p', 'Text', 'Plain paragraph', 'menu'],
  ['h1', 'Heading 1', 'Big section heading', 'tasks'],
  ['h2', 'Heading 2', 'Medium section heading', 'tasks'],
  ['h3', 'Heading 3', 'Small section heading', 'tasks'],
  ['ul', 'Bulleted list', 'A simple list', 'menu'],
  ['ol', 'Numbered list', 'Steps in order', 'menu'],
  ['todo', 'To-do', 'A checklist item', 'check'],
  ['quote', 'Quote', 'Set a note apart', 'mail'],
  ['code', 'Code', 'Commands or code', 'code'],
  ['hr', 'Divider', 'A line between sections', 'minimize'],
];

// ------------------------------------------------------------------ inline Markdown ↔ DOM

/** A link target worth keeping: web, mail and relative links only. */
function safeHref(url) {
  const trimmed = String(url || '').trim();
  if (/^(https?:|mailto:)/i.test(trimmed) || /^[./#]/.test(trimmed) || /^[\w-]+(\/[\w.-]*)*(\.\w+)?([#?].*)?$/.test(trimmed)) return trimmed;
  return null;
}

/** Inline Markdown (code, bold, italic, strikethrough, links) as DOM nodes. */
function inlineNodes(text) {
  const out = [];
  let buffer = '';
  const flush = () => { if (buffer) { out.push(document.createTextNode(buffer)); buffer = ''; } };
  for (let i = 0; i < text.length;) {
    const rest = text.slice(i);
    const before = i > 0 ? text[i - 1] : ' ';
    let m;
    if ((m = rest.match(/^`([^`\n]+)`/))) { flush(); out.push(h('code', null, m[1])); i += m[0].length; continue; }
    if ((m = rest.match(/^(\*\*|__)(?=\S)([\s\S]+?)(?<=\S)\1/)) && (m[1] === '**' || !/\w/.test(before))) { flush(); out.push(h('strong', { dataset: { mark: m[1] } }, inlineNodes(m[2]))); i += m[0].length; continue; }
    if ((m = rest.match(/^~~(?=\S)([\s\S]+?)(?<=\S)~~/))) { flush(); out.push(h('s', null, inlineNodes(m[1]))); i += m[0].length; continue; }
    if ((m = rest.match(/^\[([^\]\n]+)\]\(([^)\s]+)\)/))) {
      flush();
      const href = safeHref(m[2]);
      out.push(href ? h('a', { href, target: '_blank', rel: 'noopener noreferrer' }, inlineNodes(m[1])) : document.createTextNode(m[0]));
      i += m[0].length;
      continue;
    }
    if ((m = rest.match(/^([*_])(?=\S)([^*_\n]+?)(?<=\S)\1(?![\w*])/)) && (m[1] === '*' || !/\w/.test(before))) { flush(); out.push(h('em', { dataset: { mark: m[1] } }, inlineNodes(m[2]))); i += m[0].length; continue; }
    if (text[i] === '\n') { flush(); out.push(h('br')); i += 1; continue; }
    buffer += text[i];
    i += 1;
  }
  flush();
  return out;
}

/** The DOM of a text block back as inline Markdown. */
function inlineMarkdown(node) {
  let out = '';
  for (const child of node.childNodes) {
    if (child.nodeType === Node.TEXT_NODE) { out += child.nodeValue.replace(/ /g, ' '); continue; }
    if (child.nodeType !== Node.ELEMENT_NODE) continue;
    const tag = child.tagName;
    const inner = inlineMarkdown(child);
    if (tag === 'BR') out += '\n';
    else if (tag === 'CODE') out += child.textContent ? `\`${child.textContent}\`` : '';
    else if (!inner.trim()) out += inner;
    else if (tag === 'STRONG' || tag === 'B') { const mark = child.dataset.mark || '**'; out += `${mark}${inner}${mark}`; }
    else if (tag === 'EM' || tag === 'I') { const mark = child.dataset.mark || '*'; out += `${mark}${inner}${mark}`; }
    else if (tag === 'S' || tag === 'STRIKE' || tag === 'DEL') out += `~~${inner}~~`;
    else if (tag === 'A') out += `[${inner}](${child.getAttribute('href') || ''})`;
    else if (tag === 'DIV' || tag === 'P') out += (out && !out.endsWith('\n') ? '\n' : '') + inner;
    else out += inner;
  }
  return out;
}

// ------------------------------------------------------------------ block Markdown ↔ model

const FENCE = /^(\s*)(`{3,}|~{3,})\s*([\w+#.-]*)\s*$/;
const HR = /^\s{0,3}([-*_])(\s*\1){2,}\s*$/;

/**
 * Markdown as { frontmatter, blocks }; what is not modelled becomes a raw block. With
 * `frontmatter: false` a leading `---` block is text, not frontmatter (a prompt, SPEC-080).
 */
export function parseMarkdown(markdown, { frontmatter: withFrontmatter = true } = {}) {
  let text = String(markdown || '').replace(/\r\n?/g, '\n');
  let frontmatter = null;
  const fm = withFrontmatter && text.match(/^---\n([\s\S]*?)\n---[ \t]*(\n|$)/);
  if (fm) { frontmatter = fm[1]; text = text.slice(fm[0].length); }
  const lines = text.split('\n');
  const blocks = [];
  let open = null; // the paragraph, list item or quote later lines may continue
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    let m;
    if ((m = line.match(FENCE))) {
      const body = [];
      let j = i + 1;
      while (j < lines.length && !(lines[j].trim().startsWith(m[2][0].repeat(3)) && lines[j].trim().replace(/[`~]/g, '') === '')) { body.push(lines[j]); j += 1; }
      blocks.push({ type: 'code', lang: m[3] || '', text: body.join('\n') });
      i = j;
      open = null;
      continue;
    }
    if (!line.trim()) { open = null; continue; }
    if ((m = line.match(/^(#{1,6})\s+(.*?)\s*#*\s*$/))) { blocks.push({ type: `h${Math.min(3, m[1].length)}`, depth: m[1].length, text: m[2] }); open = null; continue; }
    if (HR.test(line) && !/^\s*[-*]\s+\S/.test(line)) { blocks.push({ type: 'hr' }); open = null; continue; }
    if ((m = line.match(/^(\s*)[-*+]\s+\[( |x|X)\]\s?(.*)$/))) { open = { type: 'todo', level: Math.floor(m[1].length / 2), checked: m[2] !== ' ', text: m[3] }; blocks.push(open); continue; }
    if ((m = line.match(/^(\s*)[-*+]\s+(.*)$/))) { open = { type: 'ul', level: Math.floor(m[1].length / 2), text: m[2] }; blocks.push(open); continue; }
    if ((m = line.match(/^(\s*)\d{1,9}[.)]\s+(.*)$/))) { open = { type: 'ol', level: Math.floor(m[1].length / 2), text: m[2] }; blocks.push(open); continue; }
    if ((m = line.match(/^\s{0,3}>\s?(.*)$/))) {
      if (open && open.type === 'quote') open.text += `\n${m[1]}`;
      else { open = { type: 'quote', text: m[1] }; blocks.push(open); }
      continue;
    }
    if (/^\s*(\||<)/.test(line)) {
      // Tables and HTML stay as they are, as one raw block up to the next blank line.
      const body = [line];
      while (i + 1 < lines.length && lines[i + 1].trim()) { body.push(lines[i + 1]); i += 1; }
      blocks.push({ type: 'raw', text: body.join('\n') });
      open = null;
      continue;
    }
    if (open && (open.type === 'p' || (LIST_TYPES.has(open.type) && /^\s+\S/.test(line)))) { open.text += `\n${line.trim()}`; continue; }
    open = { type: 'p', text: line };
    blocks.push(open);
  }
  return { frontmatter, blocks };
}

/** { frontmatter, blocks } back as Markdown. */
export function serializeMarkdown({ frontmatter, blocks }) {
  const parts = [];
  let previous = null;
  let number = 0;
  for (const block of blocks) {
    const indent = '  '.repeat(block.level || 0);
    const continued = (text, pad) => text.split('\n').join(`\n${pad}`);
    if (previous) parts.push(LIST_TYPES.has(block.type) && LIST_TYPES.has(previous.type) ? '\n' : '\n\n');
    number = block.type === 'ol' ? (previous && previous.type === 'ol' && (previous.level || 0) >= (block.level || 0) ? number + 1 : 1) : 0;
    if (block.type === 'p') parts.push(block.text);
    else if (/^h[1-3]$/.test(block.type)) parts.push(`${'#'.repeat(block.type === 'h3' ? Math.max(3, block.depth || 3) : Number(block.type[1]))} ${block.text}`);
    else if (block.type === 'ul') parts.push(`${indent}- ${continued(block.text, `${indent}  `)}`);
    else if (block.type === 'ol') parts.push(`${indent}${number}. ${continued(block.text, `${indent}   `)}`);
    else if (block.type === 'todo') parts.push(`${indent}- [${block.checked ? 'x' : ' '}] ${continued(block.text, `${indent}  `)}`);
    else if (block.type === 'quote') parts.push(block.text.split('\n').map((line) => (line ? `> ${line}` : '>')).join('\n'));
    else if (block.type === 'code') parts.push(`\`\`\`${block.lang || ''}\n${block.text}\n\`\`\``);
    else if (block.type === 'hr') parts.push('---');
    else parts.push(block.text);
    previous = block;
  }
  const body = parts.join('').replace(/\s+$/, '');
  return `${frontmatter !== null && frontmatter !== undefined ? `---\n${frontmatter}\n---\n\n` : ''}${body}\n`;
}

// ------------------------------------------------------------------ frontmatter as properties

/** Simple `key: value` frontmatter as rows; null when it is anything richer (shown raw then). */
function parseProperties(frontmatter) {
  const rows = [];
  for (const line of frontmatter.split('\n')) {
    if (!line.trim()) continue;
    const m = line.match(/^([A-Za-z_][\w-]*):[ \t]*(.*)$/);
    if (!m || /^[|>[{]/.test(m[2])) return null;
    const quoted = m[2].match(/^(["'])(.*)\1$/);
    rows.push({ key: m[1], value: quoted ? quoted[2] : m[2], quote: quoted ? quoted[1] : '' });
  }
  return rows;
}

function serializeProperties(rows) {
  return rows.map(({ key, value, quote }) => {
    const needs = quote || /^[\s>|[{&*!%@`#'"]|:\s|\s#|\s$/.test(value);
    return `${key}: ${needs ? `"${value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"` : value}`;
  }).join('\n');
}

// ------------------------------------------------------------------ caret helpers

function caretOffset(el) {
  const selection = window.getSelection();
  if (!selection.rangeCount) return 0;
  const range = selection.getRangeAt(0);
  if (!el.contains(range.startContainer)) return 0;
  const before = document.createRange();
  before.selectNodeContents(el);
  before.setEnd(range.startContainer, range.startOffset);
  return before.toString().length;
}

function placeCaret(el, atEnd = false) {
  el.focus({ preventScroll: true });
  const range = document.createRange();
  range.selectNodeContents(el);
  range.collapse(!atEnd);
  const selection = window.getSelection();
  selection.removeAllRanges();
  selection.addRange(range);
  el.scrollIntoView({ block: 'nearest' });
}

const collapsedIn = (el) => { const s = window.getSelection(); return s.rangeCount && s.isCollapsed && el.contains(s.anchorNode); };

// ------------------------------------------------------------------ the editor

/**
 * createMarkdownEditor({ markdown, readOnly, frontmatter, onChange }) → { el, getMarkdown(),
 * setMarkdown(md), focus(), setSource(on) }. `onChange` runs after every edit; `frontmatter: false`
 * shows a leading `---` block as text (a prompt is a message, not a document with properties).
 */
export function createMarkdownEditor({ markdown = '', readOnly = false, frontmatter: withFrontmatter = true, onChange = () => {} } = {}) {
  let properties = null; // [{key, value, quote}] or null
  let rawFrontmatter = null; // frontmatter too rich for properties, edited as text
  let hasFrontmatter = false;
  let sourceMode = false;
  const props = h('div', { class: 'md-props' });
  const blocksEl = h('div', { class: 'md-blocks' });
  const source = h('textarea', { class: 'md-source', spellcheck: 'false', 'aria-label': 'Markdown source', hidden: true, readOnly });
  const toolbar = h('div', { class: 'md-toolbar', role: 'toolbar', 'aria-label': 'Format', hidden: true });
  const el = h('div', { class: ['md-editor', readOnly && 'md-readonly'] }, props, blocksEl, source, toolbar);
  const changed = () => onChange();

  // ---- blocks
  function textEl(block) { return block.querySelector('.md-text'); }

  function makeBlock(model) {
    const type = model.type;
    const block = h('div', { class: 'md-block', dataset: { type } });
    if (model.level) block.dataset.level = String(Math.min(4, model.level));
    if (model.depth) block.dataset.depth = String(model.depth);
    if (type === 'hr') {
      block.append(h('hr'));
      if (!readOnly) block.tabIndex = 0;
      block.setAttribute('aria-label', 'Divider');
      return block;
    }
    if (type === 'code' || type === 'raw') {
      const lang = type === 'code' ? h('input', { class: 'md-lang', value: model.lang || '', placeholder: 'language', 'aria-label': 'Code language', readOnly, spellcheck: 'false', onInput: changed }) : h('span', { class: 'md-lang md-raw-label' }, 'Markdown');
      block.append(lang, h('pre', { class: 'md-text md-code', contenteditable: readOnly ? 'false' : 'plaintext-only', spellcheck: 'false', 'aria-label': type === 'code' ? 'Code' : 'Markdown' }, model.text || ''));
      return block;
    }
    const text = h('div', { class: 'md-text', contenteditable: readOnly ? 'false' : 'true', dataset: { placeholder: PLACEHOLDER[type] || '' } });
    text.append(...inlineNodes(model.text || ''));
    if (type === 'todo') {
      block.append(h('input', { type: 'checkbox', class: 'md-check', checked: Boolean(model.checked), disabled: readOnly, 'aria-label': 'Done', onChange: () => { block.classList.toggle('md-done', block.querySelector('.md-check').checked); changed(); } }), text);
      block.classList.toggle('md-done', Boolean(model.checked));
    } else {
      block.append(text);
    }
    return block;
  }

  function modelOf(block) {
    const type = block.dataset.type;
    const level = Number(block.dataset.level || 0);
    if (type === 'hr') return { type };
    if (type === 'code') return { type, lang: block.querySelector('.md-lang').value.trim(), text: textEl(block).textContent.replace(/\n$/, '') };
    if (type === 'raw') return { type, text: textEl(block).textContent.replace(/\n$/, '') };
    const text = inlineMarkdown(textEl(block)).replace(/\n+$/, '');
    const model = { type, text, level };
    if (block.dataset.depth) model.depth = Number(block.dataset.depth);
    if (type === 'todo') model.checked = block.querySelector('.md-check').checked;
    return model;
  }

  /** Turns a block into another kind, keeping its text; focuses it. */
  function setType(block, type, { lang = '' } = {}) {
    const old = modelOf(block);
    const text = old.type === 'code' || old.type === 'raw' ? old.text : old.text || '';
    const next = makeBlock({ type, text: type === 'hr' ? '' : text, level: LIST_TYPES.has(type) ? old.level || 0 : 0, lang });
    block.replaceWith(next);
    if (type === 'hr') {
      const after = insertAfter(next, { type: 'p', text: '' });
      placeCaret(textEl(after));
    } else {
      placeCaret(textEl(next), true);
    }
    changed();
    return next;
  }

  function insertAfter(block, model) {
    const next = makeBlock(model);
    block.after(next);
    return next;
  }

  function renderBlocks(models) {
    blocksEl.replaceChildren(...(models.length ? models : [{ type: 'p', text: '' }]).map(makeBlock));
  }

  // ---- properties (frontmatter)
  function renderProperties() {
    props.replaceChildren();
    if (!hasFrontmatter) return;
    if (properties) {
      props.append(h('dl', { class: 'md-prop-list' }, properties.flatMap((row) => [
        h('dt', null, row.key),
        h('dd', null, h(row.key === 'description' ? 'textarea' : 'input', { class: ['md-prop', row.key === 'description' && 'md-prop-long'], value: row.value, rows: row.key === 'description' ? 2 : null, readOnly, 'aria-label': row.key, spellcheck: row.key === 'description' ? 'true' : 'false',
          onInput: (event) => { row.value = event.target.value.replace(/\n+/g, ' '); changed(); } })),
      ])));
    } else {
      props.append(h('label', { class: 'md-prop-raw' }, h('span', { class: 'md-raw-label' }, 'Frontmatter'),
        h('textarea', { class: 'md-source md-frontmatter', value: rawFrontmatter, rows: Math.min(12, rawFrontmatter.split('\n').length + 1), readOnly, spellcheck: 'false', onInput: (event) => { rawFrontmatter = event.target.value; changed(); } })));
    }
  }

  // ---- reading and writing Markdown
  function setMarkdown(markdown) {
    const parsed = parseMarkdown(markdown, { frontmatter: withFrontmatter });
    hasFrontmatter = parsed.frontmatter !== null;
    properties = hasFrontmatter ? parseProperties(parsed.frontmatter) : null;
    rawFrontmatter = hasFrontmatter && !properties ? parsed.frontmatter : null;
    renderProperties();
    renderBlocks(parsed.blocks);
    source.value = markdown;
  }

  function getMarkdown() {
    if (sourceMode) return source.value;
    const frontmatter = !hasFrontmatter ? null : properties ? serializeProperties(properties) : rawFrontmatter;
    return serializeMarkdown({ frontmatter, blocks: [...blocksEl.children].map(modelOf) });
  }

  /** Shows the Markdown source (or the formatted blocks again). */
  function setSource(on) {
    if (on === sourceMode) return;
    if (on) { source.value = getMarkdown(); sourceMode = true; } else { sourceMode = false; setMarkdown(source.value); }
    props.hidden = on;
    blocksEl.hidden = on;
    source.hidden = !on;
    if (on) source.focus();
  }
  source.addEventListener('input', changed);

  // ---- slash menu
  function openBlockMenu(block, text) {
    const rect = (window.getSelection().rangeCount ? window.getSelection().getRangeAt(0).getBoundingClientRect() : null) || text.getBoundingClientRect();
    openMenu({ x: rect.left || text.getBoundingClientRect().left, y: (rect.bottom || text.getBoundingClientRect().bottom) + 4 },
      BLOCK_MENU.map(([type, label, description, iconName]) => ({ label, description, icon: iconName, onSelect: () => {
        text.replaceChildren();
        setType(block, type);
      } })));
  }

  // ---- shortcuts typed at a block's start
  const SHORTCUTS = [[/^###\s$/, 'h3'], [/^##\s$/, 'h2'], [/^#\s$/, 'h1'], [/^[-*+]\s$/, 'ul'], [/^1[.)]\s$/, 'ol'], [/^\[[ xX]?\]\s$/, 'todo'], [/^>\s$/, 'quote']];

  function onInput(event) {
    const text = event.target.closest('.md-text');
    if (!text) return;
    const block = text.closest('.md-block');
    // A text left with only a stray <br> is empty (so its placeholder shows).
    if (text.textContent === '' && text.querySelector('br') && text.childNodes.length === 1) text.replaceChildren();
    if (TEXT_TYPES.has(block.dataset.type) && event.inputType === 'insertText') {
      const content = text.textContent;
      if (content === '/' && block.dataset.type === 'p') { openBlockMenu(block, text); return; }
      if (event.data === ' ' && caretOffset(text) <= 4) {
        const head = content.slice(0, caretOffset(text)).replace(/ /g, ' ');
        for (const [pattern, type] of SHORTCUTS) {
          if (!pattern.test(head) || block.dataset.type === type) continue;
          const checked = /x/i.test(head);
          stripStart(text, head.length);
          const next = setType(block, type);
          if (type === 'todo' && checked) { next.querySelector('.md-check').checked = true; next.classList.add('md-done'); }
          placeCaret(textEl(next));
          return;
        }
      }
    }
    changed();
  }

  /** Removes the first n characters of a text block (a typed shortcut). */
  function stripStart(text, n) {
    let left = n;
    const walker = document.createTreeWalker(text, NodeFilter.SHOW_TEXT);
    while (left > 0 && walker.nextNode()) {
      const node = walker.currentNode;
      const cut = Math.min(left, node.nodeValue.length);
      node.nodeValue = node.nodeValue.slice(cut);
      left -= cut;
    }
  }

  // ---- keyboard
  function onKeydown(event) {
    const mod = event.metaKey || event.ctrlKey;
    const block = event.target.closest('.md-block');
    if (!block) return;
    const type = block.dataset.type;
    if (type === 'hr') {
      if (event.key === 'Backspace' || event.key === 'Delete') {
        event.preventDefault();
        const neighbour = block.previousElementSibling || block.nextElementSibling;
        block.remove();
        if (!blocksEl.children.length) renderBlocks([]);
        const target = neighbour && textEl(neighbour) ? textEl(neighbour) : textEl(blocksEl.firstElementChild);
        if (target) placeCaret(target, true);
        changed();
      } else if (event.key === 'Enter') {
        event.preventDefault();
        placeCaret(textEl(insertAfter(block, { type: 'p', text: '' })));
        changed();
      }
      return;
    }
    const text = event.target.closest('.md-text');
    if (!text) return;

    if (mod && !event.altKey) {
      const key = event.key.toLowerCase();
      if (TEXT_TYPES.has(type) && (key === 'b' || key === 'i' || key === 'e' || key === 'k' || (key === 'x' && event.shiftKey))) {
        event.preventDefault();
        format({ b: 'bold', i: 'italic', e: 'code', k: 'link', x: 'strike' }[key]);
        return;
      }
      if (key === 'enter' && (type === 'code' || type === 'raw')) {
        event.preventDefault();
        placeCaret(textEl(insertAfter(block, { type: 'p', text: '' })));
        changed();
        return;
      }
    }

    if (type === 'code' || type === 'raw') {
      if (event.key === 'Tab' && !event.shiftKey) { event.preventDefault(); document.execCommand('insertText', false, '  '); return; }
      if (event.key === 'ArrowDown' && caretOffset(text) >= text.textContent.length && !block.nextElementSibling) {
        event.preventDefault();
        placeCaret(textEl(insertAfter(block, { type: 'p', text: '' })));
        return;
      }
      if (event.key === 'Backspace' && !text.textContent && collapsedIn(text)) { event.preventDefault(); setType(block, 'p'); return; }
      navigate(event, block, text);
      return;
    }

    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      const content = text.textContent;
      const fence = content.match(/^```([\w+#.-]*)$/);
      if (type === 'p' && fence) { text.replaceChildren(); setType(block, 'code', { lang: fence[1] }); return; }
      if (type === 'p' && /^(---|\*\*\*|___)$/.test(content.trim())) { text.replaceChildren(); setType(block, 'hr'); return; }
      if (!content && (LIST_TYPES.has(type) || type === 'quote')) {
        if (Number(block.dataset.level || 0) > 0) { indent(block, -1); return; }
        setType(block, 'p');
        return;
      }
      // Split at the caret: what follows moves to a new block (a list goes on as a list).
      const selection = window.getSelection();
      const range = selection.getRangeAt(0);
      range.deleteContents();
      const tail = document.createRange();
      tail.setStart(range.startContainer, range.startOffset);
      tail.setEnd(text, text.childNodes.length);
      const moved = tail.extractContents();
      const nextType = LIST_TYPES.has(type) ? type : 'p';
      const next = insertAfter(block, { type: nextType, text: '', level: LIST_TYPES.has(type) ? Number(block.dataset.level || 0) : 0 });
      textEl(next).append(moved);
      if (textEl(next).textContent === '') textEl(next).replaceChildren();
      if (text.textContent === '' && text.querySelector('br')) text.replaceChildren();
      placeCaret(textEl(next));
      changed();
      return;
    }
    if (event.key === 'Enter' && event.shiftKey) { event.preventDefault(); document.execCommand('insertLineBreak'); return; }

    if (event.key === 'Tab' && LIST_TYPES.has(type)) { event.preventDefault(); indent(block, event.shiftKey ? -1 : 1); return; }

    if (event.key === 'Backspace' && collapsedIn(text) && caretOffset(text) === 0) {
      if (type !== 'p') {
        event.preventDefault();
        if (LIST_TYPES.has(type) && Number(block.dataset.level || 0) > 0) { indent(block, -1); return; }
        placeCaret(textEl(setType(block, 'p')));
        return;
      }
      const previous = block.previousElementSibling;
      if (!previous) return;
      event.preventDefault();
      if (previous.dataset.type === 'hr') { previous.remove(); changed(); return; }
      const target = textEl(previous);
      if (previous.dataset.type === 'code' || previous.dataset.type === 'raw') {
        if (!text.textContent) { block.remove(); placeCaret(target, true); changed(); }
        return;
      }
      // Merge into the block above, the caret where they meet.
      const marker = document.createRange();
      marker.selectNodeContents(target);
      marker.collapse(false);
      const join = document.createTextNode('');
      target.append(join, ...text.childNodes);
      block.remove();
      const caret = document.createRange();
      caret.setStartAfter(join);
      caret.collapse(true);
      target.focus({ preventScroll: true });
      const selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(caret);
      changed();
      return;
    }
    navigate(event, block, text);
  }

  /** Arrow keys past a block's first or last line move to the block above or below. */
  function navigate(event, block, text) {
    if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return;
    const up = event.key === 'ArrowUp';
    const offset = caretOffset(text);
    const atEdge = up ? !text.textContent.slice(0, offset).includes('\n') && onFirstLine(text) : !text.textContent.slice(offset).includes('\n') && onLastLine(text);
    if (!atEdge) return;
    let sibling = up ? block.previousElementSibling : block.nextElementSibling;
    while (sibling && sibling.dataset.type === 'hr') sibling = up ? sibling.previousElementSibling : sibling.nextElementSibling;
    if (!sibling) return;
    event.preventDefault();
    placeCaret(textEl(sibling), up);
  }

  const caretRect = () => { const s = window.getSelection(); return s.rangeCount ? s.getRangeAt(0).getBoundingClientRect() : null; };
  function onFirstLine(text) { const r = caretRect(); return !r || !r.height || r.top - text.getBoundingClientRect().top < r.height; }
  function onLastLine(text) { const r = caretRect(); return !r || !r.height || text.getBoundingClientRect().bottom - r.bottom < r.height; }

  function indent(block, step) {
    const level = Math.max(0, Math.min(4, Number(block.dataset.level || 0) + step));
    if (level) block.dataset.level = String(level); else delete block.dataset.level;
    changed();
  }

  // ---- inline formatting
  function format(kind) {
    const selection = window.getSelection();
    if (!selection.rangeCount) return;
    if (kind === 'bold') document.execCommand('bold');
    else if (kind === 'italic') document.execCommand('italic');
    else if (kind === 'strike') document.execCommand('strikeThrough');
    else if (kind === 'code') {
      const range = selection.getRangeAt(0);
      const inCode = range.commonAncestorContainer.parentElement?.closest('code');
      if (inCode) { inCode.replaceWith(document.createTextNode(inCode.textContent)); }
      else if (!range.collapsed) { const code = h('code', null, range.toString()); range.deleteContents(); range.insertNode(code); }
    } else if (kind === 'link') {
      const range = selection.getRangeAt(0).cloneRange();
      const host = range.commonAncestorContainer.parentElement?.closest('.md-text');
      const input = h('input', { class: 'input', type: 'url', placeholder: 'https://…', 'aria-label': 'Link address' });
      openModal({ title: 'Link', body: input, actions: [{ label: 'Cancel', value: false }, { label: 'Add link', kind: 'primary', run: () => {
        const href = safeHref(input.value);
        if (!href) { input.setAttribute('aria-invalid', 'true'); return false; }
        if (host) host.focus({ preventScroll: true });
        const s = window.getSelection();
        s.removeAllRanges();
        s.addRange(range);
        if (range.collapsed) range.insertNode(h('a', { href, target: '_blank', rel: 'noopener noreferrer' }, href));
        else document.execCommand('createLink', false, href);
        changed();
        return true;
      } }] });
      return;
    }
    changed();
  }

  // ---- the selection toolbar
  const TOOLS = [['bold', 'B', 'Bold (⌘B)'], ['italic', 'I', 'Italic (⌘I)'], ['strike', 'S', 'Strikethrough (⌘⇧X)'], ['code', '</>', 'Code (⌘E)'], ['link', 'Link', 'Link (⌘K)']];
  toolbar.append(...TOOLS.map(([kind, label, title]) => h('button', { type: 'button', class: ['md-tool', `md-tool-${kind}`], title, 'aria-label': title, onMousedown: (event) => event.preventDefault(), onClick: () => format(kind) }, label)));
  let toolbarFrame = 0;
  function placeToolbar() {
    toolbarFrame = 0;
    const selection = window.getSelection();
    const text = selection.rangeCount && !selection.isCollapsed ? selection.getRangeAt(0).commonAncestorContainer : null;
    const host = text && (text.nodeType === 1 ? text : text.parentElement)?.closest('.md-text');
    if (readOnly || !host || !blocksEl.contains(host) || !TEXT_TYPES.has(host.closest('.md-block').dataset.type)) { toolbar.hidden = true; return; }
    const rect = selection.getRangeAt(0).getBoundingClientRect();
    const box = el.getBoundingClientRect();
    toolbar.hidden = false;
    toolbar.style.left = `${Math.max(0, rect.left - box.left + rect.width / 2 - toolbar.offsetWidth / 2)}px`;
    toolbar.style.top = `${rect.top - box.top - toolbar.offsetHeight - 6}px`;
  }
  const onSelection = () => { if (!toolbarFrame) toolbarFrame = requestAnimationFrame(placeToolbar); };

  // ---- paste: text only; several lines become blocks
  function onPaste(event) {
    const text = event.target.closest('.md-text');
    if (!text) return;
    event.preventDefault();
    const plain = (event.clipboardData && event.clipboardData.getData('text/plain')) || '';
    const block = text.closest('.md-block');
    if (!plain.includes('\n') || block.dataset.type === 'code' || block.dataset.type === 'raw') { document.execCommand('insertText', false, plain); changed(); return; }
    const parsed = parseMarkdown(plain).blocks;
    let after = block;
    if (!text.textContent.trim() && block.dataset.type === 'p') { after = block.previousElementSibling; block.remove(); }
    let last = null;
    for (const model of parsed) {
      last = makeBlock(model);
      if (after) after.after(last); else blocksEl.prepend(last);
      after = last;
    }
    if (last && textEl(last)) placeCaret(textEl(last), true);
    changed();
  }

  if (!readOnly) {
    blocksEl.addEventListener('keydown', onKeydown);
    blocksEl.addEventListener('input', onInput);
    blocksEl.addEventListener('paste', onPaste);
    document.addEventListener('selectionchange', onSelection);
    // Clicking below the last block continues writing there.
    blocksEl.addEventListener('mousedown', (event) => {
      if (event.target !== blocksEl) return;
      const last = blocksEl.lastElementChild;
      event.preventDefault();
      if (last && last.dataset.type === 'p' && !textEl(last).textContent) placeCaret(textEl(last));
      else placeCaret(textEl(insertAfter(last, { type: 'p', text: '' })));
    });
  }

  setMarkdown(markdown);
  return {
    el,
    getMarkdown,
    setMarkdown,
    setSource,
    get sourceMode() { return sourceMode; },
    focus() { const first = blocksEl.querySelector('.md-text'); if (first) placeCaret(first, true); },
    destroy() { document.removeEventListener('selectionchange', onSelection); cancelAnimationFrame(toolbarFrame); },
  };
}
