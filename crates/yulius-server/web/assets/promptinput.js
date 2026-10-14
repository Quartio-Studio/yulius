// The new-task prompt with its files inside the text (SPEC-124, ADR-0101). A file pasted or
// dropped lands at the caret (the drop point) as an inline atom: an image as a thumbnail, another
// file as a chip with its name. An atom is one character for editing: arrows step over it,
// Backspace and Delete remove it, a selection takes it along. It moves by dragging it, or by
// cutting (⌘/Ctrl+X) and pasting (⌘/Ctrl+V) it, the keyboard way. The text is plain: pasted text
// stays text, formatting commands are refused, and the Markdown keys of mdinput.js work as in a
// textarea.
//
// The element is a `contenteditable` text box holding text nodes (`white-space: pre-wrap`) and
// atoms (`<span contenteditable="false">`), with one `<br>` at its end so a last empty line shows.
// Its model is a string where each atom is U+FFFC, with the atoms in order, so the pure edits of
// mdinput.js apply as they are. Typing stays the browser's own; every other edit (Enter, paste,
// drop, cut, a move, a Markdown key) is made on the model and drawn again. The undo history is
// its own: each edit is a step (typing a word is one), and ⌘/Ctrl+Z, ⇧⌘Z / Ctrl+Y and the
// browser's Undo walk it, files included.
//
// `value` is the prompt as sent: each file at its place as the mention `@'<path>'`.

import { h, icon } from './dom.js';
import { enterEdit, tabEdit, wrapEdit } from './mdinput.js';
import { IS_MAC } from './shortcuts.js';
import { mentionOf, splitMentions } from './promptparts.js';

/** One atom in the model's text. */
export const ATOM = '\uFFFC';
/** Steps the undo history keeps. */
const HISTORY_MAX = 100;
/** Typing within this long of the last typing joins its undo step (within a word). */
const TYPING_MS = 1000;

const countAtoms = (text) => text.split(ATOM).length - 1;
const sameAtoms = (a, b) => a.length === b.length && a.every((atom, i) => atom.path === b[i].path && atom.pending === b[i].pending);

/**
 * A prompt text box. Options:
 *   label, placeholder, keyshortcuts, maxLength   its accessible name, hint, keys and length cap
 *   face(path)        how a file looks: `{ name, image? }` (`image` a URL to draw it from)
 *   known(path)       whether a pasted mention names a file of this prompt (it becomes the file)
 *   onChange()        after every change of the text, by the person or by an edit
 *   onSubmit()        ⌘/Ctrl+Enter
 *   onOpen(path)      a file is double-clicked (an image previews)
 */
export function createPromptInput({ label, placeholder = '', keyshortcuts, maxLength = 100000, face, known = () => false, onChange = () => {}, onSubmit = () => {}, onOpen = () => {} }) {
  const el = h('div', {
    class: 'composer-prompt prompt-input', role: 'textbox', 'aria-multiline': 'true', 'aria-label': label,
    'aria-keyshortcuts': keyshortcuts, tabindex: '0', spellcheck: 'true', 'data-placeholder': placeholder,
  });
  el.contentEditable = 'true';
  let readOnly = false;
  let disabled = false;
  let dragged = null; // the file being dragged, while it is
  let moving = null; // `[start, end]` of this box's own selection or file being dragged
  let pendingCount = 0;
  const history = { states: [], index: -1, kind: '', at: 0 };

  // ---------------------------------------------------------------- the DOM and the model

  const isAtom = (node) => node.nodeType === 1 && node.classList.contains('pi-atom');
  const atomData = (node) => (node.dataset.pending ? { pending: node.dataset.pending } : { path: node.dataset.path });

  function atomEl(atom) {
    let node;
    if (atom.pending) {
      node = h('span', { class: 'pi-atom pi-pending', 'aria-label': 'A file uploading', dataset: { pending: atom.pending } },
        h('span', { class: 'pi-spinner', 'aria-hidden': 'true' }), h('span', { class: 'pi-file-name' }, 'Uploading…'));
    } else {
      const look = face(atom.path) || { name: atom.path.split('/').pop() };
      node = h('span', { class: ['pi-atom', look.image ? 'pi-image' : 'pi-file'], dataset: { path: atom.path }, title: look.name });
      if (look.image) node.append(h('img', { src: look.image, alt: look.name, draggable: 'false' }));
      else node.append(icon('file', { size: 14 }), h('span', { class: 'pi-file-name' }, look.name));
      node.draggable = true;
    }
    node.contentEditable = 'false';
    return node;
  }

  /** The node after which nothing else is: the `<br>` that ends the box, when it is one. */
  function lastLeaf() {
    let node = el.lastChild;
    while (node && node.lastChild && !isAtom(node)) node = node.lastChild;
    return node;
  }

  /**
   * The model as the DOM holds it, `{ plain, atoms, offsets }`, with each DOM point of `points`
   * (`[node, offset]`) as an offset into `plain`. Reads any DOM the browser made: a block starts
   * a line, a `<br>` is one (but the last one), a stray U+FFFC is dropped.
   */
  function scan(points = []) {
    let plain = '';
    const atoms = [];
    const offsets = points.map(() => null);
    const last = lastLeaf();
    const at = (node, offset) => points.forEach(([n, o], i) => { if (offsets[i] === null && n === node && o === offset) offsets[i] = plain.length; });
    const walk = (node) => {
      node.childNodes.forEach((child, i) => {
        at(node, i);
        if (child.nodeType === 3) {
          points.forEach(([n, o], k) => { if (offsets[k] === null && n === child) offsets[k] = plain.length + child.data.slice(0, o).replaceAll(ATOM, '').length; });
          plain += child.data.replaceAll(ATOM, '');
        } else if (child.nodeType === 1) {
          if (isAtom(child)) {
            points.forEach(([n], k) => { if (offsets[k] === null && child.contains(n)) offsets[k] = plain.length; });
            plain += ATOM;
            atoms.push(atomData(child));
          } else if (child.tagName === 'BR') {
            if (child !== last) plain += '\n';
          } else {
            if (/^(DIV|P|LI)$/.test(child.tagName) && plain && !plain.endsWith('\n')) plain += '\n';
            walk(child);
          }
        }
      });
      at(node, node.childNodes.length);
    };
    walk(el);
    return { plain, atoms, offsets };
  }

  /** Whether the DOM is as `draw` makes it: text, atoms and the closing `<br>` only. */
  function canonical() {
    const nodes = [...el.childNodes];
    const end = nodes.pop();
    return Boolean(end) && end.nodeName === 'BR' && nodes.every((n) => (n.nodeType === 3 && !n.data.includes(ATOM)) || (n.nodeType === 1 && isAtom(n)));
  }

  /** Draws the model. */
  function draw(plain, atoms) {
    const nodes = [];
    plain.split(ATOM).forEach((text, i) => {
      if (i > 0) nodes.push(atomEl(atoms[i - 1]));
      if (text) nodes.push(document.createTextNode(text));
    });
    nodes.push(document.createElement('br'));
    el.replaceChildren(...nodes);
    el.classList.toggle('pi-empty', !plain);
  }

  /** The DOM point of model offset `offset` (in a drawn DOM). */
  function pointAt(offset) {
    let count = 0;
    const nodes = [...el.childNodes];
    for (let i = 0; i < nodes.length; i += 1) {
      const node = nodes[i];
      if (node.nodeType === 3) {
        if (offset <= count + node.data.length) return [node, offset - count];
        count += node.data.length;
      } else if (isAtom(node)) {
        if (offset <= count) return [el, i];
        count += 1;
      } else if (node.nodeName === 'BR') {
        return [el, i];
      }
    }
    return [el, nodes.length];
  }

  /** The selection as model offsets `[start, end]` (start ≤ end), or the end when it is not here. */
  function selection() {
    const sel = window.getSelection();
    if (!sel || !sel.rangeCount || !el.contains(sel.anchorNode)) { const { plain } = scan(); return [plain.length, plain.length]; }
    const range = sel.getRangeAt(0);
    const { offsets: [a, b], plain } = scan([[range.startContainer, range.startOffset], [range.endContainer, range.endOffset]]);
    return [a ?? plain.length, b ?? plain.length];
  }

  function select(start, end = start) {
    const sel = window.getSelection();
    if (!sel) return;
    const range = document.createRange();
    range.setStart(...pointAt(start));
    range.setEnd(...pointAt(end));
    sel.removeAllRanges();
    sel.addRange(range);
  }

  // ---------------------------------------------------------------- history and edits

  function record(kind) {
    const { plain, atoms } = scan();
    const [start, end] = el.contains(document.activeElement) || document.activeElement === el ? selection() : [plain.length, plain.length];
    const state = { plain, atoms, start, end };
    const current = history.states[history.index];
    const now = Date.now();
    if (current && current.plain === plain && sameAtoms(current.atoms, atoms)) {
      Object.assign(current, { start, end });
    } else if (current && kind === 'type' && history.kind === 'type' && now - history.at < TYPING_MS && !/\s$/.test(current.plain.slice(0, current.end))) {
      history.states[history.index] = state;
    } else {
      history.states.splice(history.index + 1);
      history.states.push(state);
      if (history.states.length > HISTORY_MAX) history.states.shift();
      history.index = history.states.length - 1;
    }
    history.kind = kind;
    history.at = now;
  }

  function restore(state) {
    draw(state.plain, state.atoms);
    select(state.start, state.end);
    history.kind = '';
    onChange();
  }

  function undo() {
    if (history.index <= 0) return;
    history.index -= 1;
    restore(history.states[history.index]);
  }

  function redo() {
    if (history.index >= history.states.length - 1) return;
    history.index += 1;
    restore(history.states[history.index]);
  }

  /**
   * Replaces model offsets `start`–`end` with `text` (U+FFFC for each of `atoms`), the caret then
   * at `caret` (after the insert by default; `[a, b]` selects): one undo step. What would pass the
   * length cap is cut, but files never are (`files`: the insert is uploaded files).
   */
  function replace(start, end, text, atoms = [], caret = null, { files = false } = {}) {
    const { plain, atoms: all } = scan();
    const room = maxLength - (plain.length - (end - start));
    if (!files && text.length > room) {
      text = text.slice(0, Math.max(0, room));
      atoms = atoms.slice(0, countAtoms(text));
    }
    const before = countAtoms(plain.slice(0, start));
    const next = plain.slice(0, start) + text + plain.slice(end);
    draw(next, [...all.slice(0, before), ...atoms, ...all.slice(before + countAtoms(plain.slice(start, end)))]);
    const [a, b] = Array.isArray(caret) ? caret : [caret ?? start + text.length, caret ?? start + text.length];
    select(a, b);
    record('edit');
    if (el.isConnected) onChange();
  }

  /**
   * Placeholder `id` becomes `files` (none: it goes), in the box and in every step of the history
   * that holds it, so it is never a step of its own and undo never shows it again. False when the
   * box no longer holds it.
   */
  function swapPending(id, files) {
    const swap = (state) => {
      const i = state.atoms.findIndex((atom) => atom.pending === id);
      if (i === -1) return null;
      let pos = -1;
      for (let seen = 0; seen <= i; seen += 1) pos = state.plain.indexOf(ATOM, pos + 1);
      state.plain = state.plain.slice(0, pos) + ATOM.repeat(files.length) + state.plain.slice(pos + 1);
      state.atoms = [...state.atoms.slice(0, i), ...files, ...state.atoms.slice(i + 1)];
      const shift = (o) => (o > pos ? o + files.length - 1 : o);
      state.start = shift(state.start);
      state.end = shift(state.end);
      return pos;
    };
    for (const state of history.states) swap(state);
    const focused = document.activeElement === el;
    const { plain, atoms } = scan();
    const [start, end] = focused ? selection() : [plain.length, plain.length];
    const live = { plain, atoms, start, end };
    if (swap(live) === null) return false;
    draw(live.plain, live.atoms);
    if (focused) select(live.start, live.end);
    if (el.isConnected) onChange();
    return true;
  }

  /** The model for pasted text: a mention of a file of this prompt becomes that file again. */
  function fromText(text) {
    let plain = '';
    const atoms = [];
    for (const piece of splitMentions(text.replaceAll(ATOM, ''))) {
      if ('path' in piece && known(piece.path)) { plain += ATOM; atoms.push({ path: piece.path }); } else plain += 'text' in piece ? piece.text : piece.mention;
    }
    return { plain, atoms };
  }

  /** `plain` from `start` to `end` as text: each file its mention. */
  function asText(plain, atoms, start = 0, end = plain.length) {
    let index = countAtoms(plain.slice(0, start));
    return plain.slice(start, end).replace(/\uFFFC/g, () => {
      const atom = atoms[index++];
      return atom && atom.path ? mentionOf(atom.path) : '';
    });
  }

  const editable = () => !readOnly && !disabled;

  // ---------------------------------------------------------------- the person's input

  el.addEventListener('beforeinput', (event) => {
    if (!editable()) { event.preventDefault(); return; }
    const type = event.inputType;
    if (type === 'historyUndo' || type === 'historyRedo') {
      event.preventDefault();
      if (type === 'historyUndo') undo(); else redo();
    } else if (type.startsWith('format')) {
      event.preventDefault();
    } else if (type === 'insertParagraph' || type === 'insertLineBreak') {
      event.preventDefault();
      const [start, end] = selection();
      replace(start, end, '\n');
    } else if ((type === 'insertText' || type === 'insertReplacementText') && event.data && /[\n\r]/.test(event.data)) {
      event.preventDefault();
      const [start, end] = selection();
      replace(start, end, event.data.replace(/\r\n?/g, '\n').replaceAll(ATOM, ''));
    } else if (type === 'insertFromDrop' || type === 'deleteByDrag' || type === 'insertFromPaste' || type === 'insertFromPasteAsQuotation') {
      // Drops and pastes are made by the handlers below.
      event.preventDefault();
    }
  });

  el.addEventListener('input', (event) => {
    if (event.isComposing) return;
    if (!canonical()) {
      const [start, end] = selection();
      const { plain, atoms } = scan();
      draw(plain, atoms);
      select(start, end);
    } else {
      el.classList.toggle('pi-empty', !scan().plain);
    }
    record(event.inputType === 'insertText' ? 'type' : event.inputType || 'edit');
    onChange();
  });
  el.addEventListener('compositionend', () => { record('type'); onChange(); });

  // Esc, then Tab, leaves the text box even on a list line (no keyboard trap).
  let escaped = false;
  el.addEventListener('keydown', (event) => {
    if (event.isComposing || event.keyCode === 229 || event.defaultPrevented) return;
    const wasEscaped = escaped;
    escaped = event.key === 'Escape';
    const mod = IS_MAC ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
    const key = event.key.length === 1 ? event.key.toLowerCase() : event.key;
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      onSubmit();
      return;
    }
    if (!editable()) return;
    if (mod && !event.altKey && (key === 'z' || (!IS_MAC && key === 'y'))) {
      event.preventDefault();
      if (key === 'y' || event.shiftKey) redo(); else undo();
      return;
    }
    const { plain } = scan();
    const [s, e] = selection();
    if (event.key === 'Enter') {
      if (event.altKey) return;
      event.preventDefault();
      const edit = !event.shiftKey && s === e ? enterEdit(plain, s) : null;
      if (edit) replace(edit.start, edit.end, edit.text, [], edit.caret);
      else replace(s, e, '\n');
      return;
    }
    if (event.key === 'Tab' && !event.metaKey && !event.ctrlKey && !event.altKey && !wasEscaped) {
      const edit = tabEdit(plain, s, e, event.shiftKey);
      if (!edit || edit.text === plain.slice(edit.start, edit.end)) return;
      event.preventDefault();
      replace(edit.start, edit.end, edit.text, atomsIn(edit.start, edit.end), [edit.selStart, edit.selEnd]);
      return;
    }
    if (mod && !event.shiftKey && !event.altKey && (key === 'b' || key === 'i' || key === 'u')) {
      event.preventDefault();
      if (key === 'u') return; // no underline in Markdown
      const edit = wrapEdit(plain, s, e, key === 'b' ? '**' : '*');
      replace(edit.start, edit.end, edit.text, atomsIn(edit.start, edit.end), [edit.selStart, edit.selEnd]);
    }
  });

  /** The atoms between model offsets `start` and `end` (an edit that keeps them passes them on). */
  function atomsIn(start, end) {
    const { plain, atoms } = scan();
    const before = countAtoms(plain.slice(0, start));
    return atoms.slice(before, before + countAtoms(plain.slice(start, end)));
  }

  // Copy and cut write each file as its mention; a paste of one of this prompt's mentions is that
  // file again (the keyboard way to move one), any other text stays text.
  function copy(event, cut) {
    const [start, end] = selection();
    if (start === end || !event.clipboardData) return;
    event.preventDefault();
    const { plain, atoms } = scan();
    event.clipboardData.setData('text/plain', asText(plain, atoms, start, end));
    if (cut && editable()) replace(start, end, '');
  }
  el.addEventListener('copy', (event) => copy(event, false));
  el.addEventListener('cut', (event) => copy(event, true));
  el.addEventListener('paste', (event) => {
    event.preventDefault();
    if (!editable() || !event.clipboardData) return;
    const text = event.clipboardData.getData('text/plain').replace(/\r\n?/g, '\n');
    if (!text) return;
    const [start, end] = selection();
    const { plain, atoms } = fromText(text);
    replace(start, end, plain, atoms);
  });

  // ---------------------------------------------------------------- drag and drop

  /** The model offset under a point of the window. */
  function offsetAtPoint(x, y) {
    let node = null;
    let offset = 0;
    if (document.caretPositionFromPoint) {
      const pos = document.caretPositionFromPoint(x, y);
      if (pos) { node = pos.offsetNode; offset = pos.offset; }
    } else if (document.caretRangeFromPoint) {
      const range = document.caretRangeFromPoint(x, y);
      if (range) { node = range.startContainer; offset = range.startOffset; }
    }
    if (!node || !el.contains(node)) return scan().plain.length;
    // A point over a file: before it, or after it on its right half.
    const atom = node.nodeType === 1 && isAtom(node) ? node : node.parentElement && node.parentElement.closest('.pi-atom');
    if (atom && el.contains(atom)) {
      const { offsets: [before] } = scan([[atom, 0]]);
      const box = atom.getBoundingClientRect();
      return before + (x > box.left + box.width / 2 ? 1 : 0);
    }
    const { offsets: [at], plain } = scan([[node, offset]]);
    return at ?? plain.length;
  }

  // A file, or the selection, dragged inside the box moves; text dragged in from elsewhere is
  // inserted as plain text.
  el.addEventListener('dragstart', (event) => {
    if (!editable()) { event.preventDefault(); return; }
    const atom = event.target instanceof Element ? event.target.closest('.pi-atom') : null;
    const { plain, atoms } = scan();
    if (atom && el.contains(atom)) {
      const { offsets: [at] } = scan([[atom, 0]]);
      if (!atom.dataset.path || at === null) { event.preventDefault(); return; }
      dragged = atom;
      atom.classList.add('pi-dragging');
      moving = [at, at + 1];
    } else {
      const [start, end] = selection();
      if (start === end) return;
      moving = [start, end];
    }
    event.dataTransfer.effectAllowed = 'copyMove';
    event.dataTransfer.setData('text/plain', asText(plain, atoms, moving[0], moving[1]));
  });
  const dragDone = () => {
    if (dragged) dragged.classList.remove('pi-dragging');
    dragged = null;
    moving = null;
  };
  el.addEventListener('dragend', dragDone);
  el.addEventListener('dragover', (event) => {
    if (!moving && !(event.dataTransfer && [...event.dataTransfer.types].includes('text/plain'))) return;
    event.preventDefault();
    if (!editable()) { event.dataTransfer.dropEffect = 'none'; return; }
    event.dataTransfer.dropEffect = moving ? 'move' : 'copy';
    showPoint(event.clientX, event.clientY);
  });
  el.addEventListener('drop', (event) => {
    if (!moving && !(event.dataTransfer && [...event.dataTransfer.types].includes('text/plain'))) return;
    event.preventDefault();
    if (!editable()) { dragDone(); return; }
    const target = offsetAtPoint(event.clientX, event.clientY);
    if (moving) {
      const [start, end] = moving;
      dragDone();
      move(start, end, target);
      return;
    }
    // Text dragged in from elsewhere: plain text at the drop point.
    const text = event.dataTransfer.getData('text/plain').replace(/\r\n?/g, '\n');
    const { plain, atoms } = fromText(text);
    if (plain) replace(target, target, plain, atoms);
  });

  /**
   * Moves model offsets `start`–`end` (a file, or a selection with its files) to offset `to` (as
   * offsets before the move), selected there: one undo step.
   */
  function move(start, end, to) {
    if (to >= start && to <= end) return;
    const { plain, atoms } = scan();
    const before = countAtoms(plain.slice(0, start));
    const count = countAtoms(plain.slice(start, end));
    const piece = plain.slice(start, end);
    const rest = plain.slice(0, start) + plain.slice(end);
    const others = [...atoms.slice(0, before), ...atoms.slice(before + count)];
    const at = to > end ? to - piece.length : to;
    const into = countAtoms(rest.slice(0, at));
    draw(rest.slice(0, at) + piece + rest.slice(at), [...others.slice(0, into), ...atoms.slice(before, before + count), ...others.slice(into)]);
    el.focus({ preventScroll: true });
    select(at, at + piece.length);
    record('move');
    onChange();
  }

  /** Shows the caret where a drop at this point of the window lands. */
  function showPoint(x, y) {
    if (!editable()) return;
    if (document.activeElement !== el) el.focus({ preventScroll: true });
    select(offsetAtPoint(x, y));
  }

  el.addEventListener('dblclick', (event) => {
    const atom = event.target instanceof Element ? event.target.closest('.pi-atom') : null;
    if (atom && atom.dataset.path) onOpen(atom.dataset.path);
  });
  // A click on a file selects it, so Delete, cut or a drag takes it.
  el.addEventListener('mousedown', (event) => {
    const atom = event.target instanceof Element ? event.target.closest('.pi-atom') : null;
    if (!atom || event.detail > 1 || !el.contains(atom)) return;
    if (document.activeElement !== el) el.focus({ preventScroll: true });
    const { offsets: [at] } = scan([[atom, 0]]);
    if (at !== null) setTimeout(() => select(at, at + 1));
  });

  // ---------------------------------------------------------------- the composer's side

  function setValue(text, appended = []) {
    let plain = '';
    const atoms = [];
    for (const piece of splitMentions(String(text ?? '').replace(/\r\n?/g, '\n').replaceAll(ATOM, ''))) {
      if ('path' in piece && known(piece.path)) { plain += ATOM; atoms.push({ path: piece.path }); } else plain += 'text' in piece ? piece.text : piece.mention;
    }
    // Files this text does not hold yet (a draft from before SPEC-124) follow it.
    const missing = appended.filter((path) => !atoms.some((atom) => atom.path === path));
    if (missing.length) {
      plain = `${plain}${plain && !/\s$/.test(plain) ? '\n' : ''}${ATOM.repeat(missing.length)}`;
      atoms.push(...missing.map((path) => ({ path })));
    }
    draw(plain, atoms);
    history.states = [];
    history.index = -1;
    record('set');
  }

  return {
    el,
    /** The prompt as sent: each file at its place as `@'<path>'` (a file uploading is left out). */
    get value() { const { plain, atoms } = scan(); return asText(plain, atoms); },
    set value(text) { setValue(text); },
    /** Sets the text (mentions of known files become files; `appended` paths follow it); a new history. */
    setValue,
    /** The text without its files. */
    text() { const { plain } = scan(); return plain.replaceAll(ATOM, ''); },
    /** The paths of the files in the text, in order. */
    paths() { return scan().atoms.filter((atom) => atom.path).map((atom) => atom.path); },
    /** The model, for a preview: `{ plain, paths }` with U+FFFC for each file (an upload: none). */
    model() {
      const { plain, atoms } = scan();
      let index = 0;
      const files = [];
      const shown = plain.replace(/\uFFFC/g, () => { const atom = atoms[index++]; if (!atom.path) return ''; files.push(atom.path); return ATOM; });
      return { plain: shown, paths: files };
    },
    get readOnly() { return readOnly; },
    set readOnly(on) { readOnly = Boolean(on); sync(); },
    get disabled() { return disabled; },
    set disabled(on) { disabled = Boolean(on); sync(); },
    get hidden() { return el.hidden; },
    set hidden(on) { el.hidden = Boolean(on); },
    focus(options) { el.focus(options); },
    /**
     * Where files uploading now will land: at the caret, or at a drop's point (`event`). A
     * placeholder shows there until `insert(paths)` puts the files in its place (or at the end
     * when it was taken out meanwhile) or `cancel()` takes it away.
     */
    begin(event) {
      if (!editable()) return null;
      pendingCount += 1;
      const id = String(pendingCount);
      const [start, end] = event && Number.isFinite(event.clientX) && (event.clientX || event.clientY)
        ? [offsetAtPoint(event.clientX, event.clientY), offsetAtPoint(event.clientX, event.clientY)]
        : selection();
      replace(start, end, ATOM, [{ pending: id }], null, { files: true });
      return {
        /** The files take the placeholder's place (after the text when it was taken out). */
        insert(paths) {
          const files = paths.map((path) => ({ path }));
          if (swapPending(id, files)) return;
          const { plain } = scan();
          replace(plain.length, plain.length, ATOM.repeat(files.length), files, null, { files: true });
        },
        cancel() { swapPending(id, []); },
      };
    },
    /** Shows the caret where something dropped at this point lands. */
    showPoint,
    /** Adds files after the text, one undo step (an upload that ended in another view). */
    append(paths) {
      if (!paths.length) return;
      const { plain } = scan();
      const gap = plain && !/\s$/.test(plain) ? '\n' : '';
      replace(plain.length, plain.length, gap + ATOM.repeat(paths.length), paths.map((path) => ({ path })), null, { files: true });
    },
    /** Draws the files again (a thumbnail became available), the caret kept. */
    refresh() {
      const focused = document.activeElement === el;
      const [start, end] = focused ? selection() : [0, 0];
      const { plain, atoms } = scan();
      draw(plain, atoms);
      if (focused) select(start, end);
    },
  };

  function sync() {
    el.contentEditable = editable() ? 'true' : 'false';
    el.setAttribute('aria-readonly', String(readOnly));
    el.setAttribute('aria-disabled', String(disabled));
    el.tabIndex = disabled ? -1 : 0;
  }
}

/** Private-use characters around a file's number in text handed to the Markdown editor. */
export const FILE_MARK = '\uE000';
export const FILE_MARK_END = '\uE001';

/** `text` without the marker characters, so text cannot place a file of its own. */
export const withoutMarks = (text) => String(text).replace(/[\uE000\uE001]/g, '');

/** Replaces each file marker in `root`'s text with `face(n)` (left as it is when that is null). */
export function placeFiles(root, face) {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const found = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) if (node.data.includes(FILE_MARK)) found.push(node);
  const mark = new RegExp(`${FILE_MARK}(\\d+)${FILE_MARK_END}`, 'g');
  for (const node of found) {
    const pieces = [];
    let last = 0;
    for (const match of node.data.matchAll(mark)) {
      const piece = face(Number(match[1]));
      if (!piece) continue;
      if (match.index > last) pieces.push(document.createTextNode(node.data.slice(last, match.index)));
      pieces.push(piece);
      last = match.index + match[0].length;
    }
    if (!pieces.length) continue;
    if (last < node.data.length) pieces.push(document.createTextNode(node.data.slice(last)));
    node.replaceWith(...pieces);
  }
}
