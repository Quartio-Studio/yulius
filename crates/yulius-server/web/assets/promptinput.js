// The new-task prompt with its files inside the text (SPEC-124, ADR-0101). A file pasted or
// dropped lands at the caret (the drop point) as an inline atom, a placeholder one line high: an
// image as `Image N`, another file as a chip with its name (SPEC-150; the composer's tray shows
// the files themselves). An atom is one character for editing: arrows step over it,
// Backspace and Delete remove it, a selection takes it along. It moves by dragging it, or by
// cutting (⌘/Ctrl+X) and pasting (⌘/Ctrl+V) it, the keyboard way, and `place` puts a file at a
// point (a tray thumbnail dropped on the text, `PLACE_TYPE`) or at the caret. The text is plain: pasted text
// stays text, formatting commands are refused, and the Markdown keys of mdinput.js work as in a
// textarea.
//
// The element is a `contenteditable` text box holding text nodes (`white-space: pre-wrap`) and
// atoms (`<span contenteditable="false">`), with one `<br>` at its end so a last empty line shows.
// Its model is a string where each atom is U+FFFC, with the atoms in order, so the pure edits of
// mdinput.js apply as they are. Typing stays the browser's own; every other edit (Enter, paste,
// drop, cut, a move, a Markdown key) is made on the model and drawn again. The undo history is
// its own: each edit is a step (typing a word is one), and ⌘/Ctrl+Z, ⇧⌘Z / Ctrl+Y and the
// browser's Undo walk it, files included. Each step also keeps the files attached outside the
// text (the composer's tray, `attached`), so undo and redo bring back a removed file in its place
// and undoing a paste takes its upload away (SPEC-150).
//
// `value` is the prompt as sent: each file at its place as the mention `@'<path>'`.

import { h, icon } from './dom.js';
import { enterEdit, tabEdit, wrapEdit } from './mdinput.js';
import { IS_MAC } from './shortcuts.js';
import { mentionOf, splitMentions } from './promptparts.js';

/** One atom in the model's text. */
export const ATOM = '\uFFFC';
/** A drag carrying a file's path to put in the text (a tray thumbnail, SPEC-150). */
export const PLACE_TYPE = 'application/x-yulius-file';
/** Steps the undo history keeps. */
const HISTORY_MAX = 100;
/** Typing within this long of the last typing joins its undo step (within a word). */
const TYPING_MS = 1000;

const countAtoms = (text) => text.split(ATOM).length - 1;
const sameAtoms = (a, b) => a.length === b.length && a.every((atom, i) => atom.path === b[i].path && atom.pending === b[i].pending);
const sameList = (a, b) => a.length === b.length && a.every((item, i) => item === b[i]);
/** `list` with each of `items` it lacks added at its end. */
const withItems = (list, items) => [...list, ...items.filter((item) => !list.includes(item))];

/**
 * A prompt text box. Options:
 *   label, placeholder, keyshortcuts, maxLength   its accessible name, hint, keys and length cap
 *   face(path)        how a file looks: `{ name, label?, image? }` (`label` its placeholder's text,
 *                     the name by default; `image` true for an image)
 *   known(path)       whether a pasted mention names a file of this prompt (it becomes the file)
 *   onChange()        after every change of the text, by the person or by an edit
 *   onSubmit()        ⌘/Ctrl+Enter
 *   onOpen(path)      a file is double-clicked (an image previews)
 *   attached          `{ read(), write(list) }`: the files attached outside the text, kept in each
 *                     undo step (`read` gives a copy; `write` is called before an undone step draws)
 */
export function createPromptInput({ label, placeholder = '', keyshortcuts, maxLength = 100000, face, known = () => false, onChange = () => {}, onSubmit = () => {}, onOpen = () => {}, attached = { read: () => [], write: () => {} } }) {
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
  let kept = null; // the caret when the box last lost the keyboard (`place` puts a file there)
  const history = { states: [], index: -1, kind: '', at: 0 };
  // Steps cut off as a redo branch by a new edit: an upload pasted in one of them is in none.
  const discarded = new WeakSet();

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
      node = h('span', { class: ['pi-atom', look.image ? 'pi-image' : 'pi-file'], dataset: { path: atom.path }, title: look.name },
        icon(look.image ? 'image' : 'file', { size: 14 }), h('span', { class: 'pi-file-name' }, look.label || look.name));
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

  /**
   * Draws the model. A drag from this box under way is forgotten: its offsets may be stale, and its
   * source node is replaced, so its `dragend` would never arrive.
   */
  function draw(plain, atoms) {
    if (moving) dragDone();
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
    const files = attached.read();
    const state = { plain, atoms, start, end, files };
    const current = history.states[history.index];
    const now = Date.now();
    const sameFiles = current && sameList(current.files, files);
    if (current && current.plain === plain && sameAtoms(current.atoms, atoms) && sameFiles) {
      Object.assign(current, { start, end });
    } else if (current && sameFiles && kind === 'type' && history.kind === 'type' && now - history.at < TYPING_MS && !/\s$/.test(current.plain.slice(0, current.end))) {
      history.states[history.index] = state;
    } else {
      for (const gone of history.states.splice(history.index + 1)) discarded.add(gone);
      history.states.push(state);
      if (history.states.length > HISTORY_MAX) history.states.shift();
      history.index = history.states.length - 1;
    }
    history.kind = kind;
    history.at = now;
  }

  function restore(state) {
    attached.write([...state.files]);
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
   * that holds it, so it is never a step of its own and undo never shows it again. `items`, the
   * uploads, join the attached files of every step from `from` (the placeholder's step) on, and of
   * every step when that step fell off the history's start; the steps before it never had them,
   * nor any step when it was cut off as a redo branch (the paste was undone). False when the box
   * no longer holds the placeholder.
   */
  function swapPending(id, files, items = [], from = null) {
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
    const first = history.states.indexOf(from);
    history.states.forEach((state, i) => {
      const held = swap(state) !== null;
      const fromOn = first === -1 ? !discarded.has(from) : i >= first;
      if (items.length && (held || fromOn)) state.files = withItems(state.files, items);
    });
    const focused = document.activeElement === el;
    const { plain, atoms } = scan();
    const [start, end] = focused ? selection() : [plain.length, plain.length];
    const live = { plain, atoms, start, end };
    const at = swap(live);
    if (at === null) return false;
    if (kept) kept = kept.map((o) => (o > at ? o + files.length - 1 : o));
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
    // Typing stops at the length cap, as a textarea's maxlength does (over a selection it replaces).
    // A composition (IME, Android) cannot be stopped, so it is not counted; the server's prompt
    // limit still holds.
    if (type.startsWith('insert') && type !== 'insertParagraph' && type !== 'insertLineBreak' && type !== 'insertCompositionText' && event.data) {
      const [start, end] = selection();
      if (scan().plain.length - (end - start) + event.data.length > maxLength) { event.preventDefault(); return; }
    }
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
    // However the drag ends, even off the box or after a redraw replaced the dragged node.
    document.addEventListener('dragend', dragDone, { capture: true, once: true });
  });
  function dragDone() {
    if (dragged) dragged.classList.remove('pi-dragging');
    dragged = null;
    moving = null;
  }
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
    const placing = !moving && [...event.dataTransfer.types].includes(PLACE_TYPE) ? event.dataTransfer.getData(PLACE_TYPE) : '';
    if (placing && known(placing)) {
      place(placing, target);
      return;
    }
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

  /**
   * Puts file `path` at model offset `to` (the caret kept when the box lost the keyboard, by
   * default): its first placeholder moves there, or one is inserted when the text holds none. One
   * undo step.
   */
  function place(path, to = null) {
    if (!editable()) return;
    const { plain, atoms } = scan();
    const at = Math.min(to ?? (kept ? kept[1] : plain.length), plain.length);
    const index = atoms.findIndex((atom) => atom.path === path);
    el.focus({ preventScroll: true });
    if (index === -1) {
      replace(at, at, ATOM, [{ path }], null, { files: true });
      return;
    }
    let pos = -1;
    for (let seen = 0; seen <= index; seen += 1) pos = plain.indexOf(ATOM, pos + 1);
    move(pos, pos + 1, at);
  }

  /** Takes every placeholder of file `path` out of the text: one undo step. False when it held none. */
  function remove(path) {
    const { plain, atoms } = scan();
    if (!atoms.some((atom) => atom.path === path)) return false;
    const focused = document.activeElement === el;
    const [start, end] = focused ? selection() : [plain.length, plain.length];
    let next = '';
    const left = [];
    const gone = []; // the offsets of the placeholders taken out
    plain.split(ATOM).forEach((text, i) => {
      if (i > 0 && atoms[i - 1].path === path) gone.push(next.length + gone.length);
      else if (i > 0) { next += ATOM; left.push(atoms[i - 1]); }
      next += text;
    });
    draw(next, left);
    const shift = (o) => o - gone.filter((at) => at < o).length;
    if (focused) select(shift(start), shift(end));
    if (kept) kept = kept.map(shift);
    record('edit');
    if (el.isConnected) onChange();
    return true;
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

  // Where `place` puts a file by default: the caret the person left.
  el.addEventListener('focusout', () => { kept = selection(); });

  // A file in the selection looks chosen as a whole: its label is never painted as selected text
  // (the stylesheet), the file is marked instead (#535). Followed while the box has the keyboard.
  function markSelected() {
    // Gone with the keyboard in it: some browsers send no focusout then.
    if (!el.isConnected) { document.removeEventListener('selectionchange', markSelected); return; }
    const sel = window.getSelection();
    const range = el.contains(document.activeElement) && sel && sel.rangeCount && el.contains(sel.anchorNode) ? sel.getRangeAt(0) : null;
    for (const atom of el.querySelectorAll('.pi-atom')) atom.classList.toggle('pi-selected', Boolean(range) && !range.collapsed && range.intersectsNode(atom));
  }
  el.addEventListener('focusin', () => {
    document.addEventListener('selectionchange', markSelected);
    markSelected();
  });
  el.addEventListener('focusout', () => {
    document.removeEventListener('selectionchange', markSelected);
    for (const atom of el.querySelectorAll('.pi-selected')) atom.classList.remove('pi-selected');
  });

  // ---------------------------------------------------------------- the composer's side

  function setValue(text) {
    let plain = '';
    const atoms = [];
    for (const piece of splitMentions(String(text ?? '').replace(/\r\n?/g, '\n').replaceAll(ATOM, ''))) {
      if ('path' in piece && known(piece.path)) { plain += ATOM; atoms.push({ path: piece.path }); } else plain += 'text' in piece ? piece.text : piece.mention;
    }
    draw(plain, atoms);
    history.states = [];
    history.index = -1;
    kept = null;
    record('set');
  }

  return {
    el,
    /** The prompt as sent: each file at its place as `@'<path>'` (a file uploading is left out). */
    get value() { const { plain, atoms } = scan(); return asText(plain, atoms); },
    set value(text) { setValue(text); },
    /** Sets the text (mentions of known files become files); a new history. */
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
     * placeholder shows there until `insert(paths)` puts the files in its place (none when it was
     * taken out meanwhile: the tray keeps them) or `cancel()` takes it away.
     */
    begin(event) {
      if (!editable()) return null;
      pendingCount += 1;
      const id = String(pendingCount);
      const [start, end] = event && Number.isFinite(event.clientX) && (event.clientX || event.clientY)
        ? [offsetAtPoint(event.clientX, event.clientY), offsetAtPoint(event.clientX, event.clientY)]
        : selection();
      replace(start, end, ATOM, [{ pending: id }], null, { files: true });
      const step = history.states[history.index];
      return {
        /**
         * The files take the placeholder's place (nowhere when it was taken out); `items`, now
         * attached, are attached in every step from the paste on. When the box no longer holds the
         * placeholder, the attached files are the current step's: an undone paste brings no file
         * until it is redone.
         */
        insert(paths, items = []) {
          if (swapPending(id, paths.map((path) => ({ path })), items, step)) return;
          const current = history.states[history.index];
          if (!current || sameList(current.files, attached.read())) return;
          attached.write([...current.files]);
          if (el.isConnected) onChange();
        },
        cancel() {
          swapPending(id, []);
          // Taken out of every step (also when the box no longer held it), the placeholder's step is
          // now a copy of the one before it: not a step to undo.
          const same = (a, b) => a.plain === b.plain && sameAtoms(a.atoms, b.atoms) && sameList(a.files, b.files);
          const states = history.states;
          const kept = states.filter((state, i) => i === 0 || !same(state, states[i - 1]));
          history.index -= states.slice(0, history.index + 1).filter((state, i) => i > 0 && same(state, states[i - 1])).length;
          history.states = kept;
        },
      };
    },
    /** Shows the caret where something dropped at this point lands. */
    showPoint,
    /** Puts a file at the caret the person left (its placeholder moves there, or one is added). */
    place: (path) => place(path),
    /** Takes a file's placeholders out of the text (it left the tray), one undo step; false when none. */
    remove,
    /** Records the attached files as an undo step (they changed, the text did not). */
    commit() { record('edit'); if (el.isConnected) onChange(); },
    /** The current step keeps the attached files as they are now (not a step of its own). */
    amend() { const current = history.states[history.index]; if (current) current.files = attached.read(); },
    /** Files attached outside any edit (an upload that ended elsewhere) are attached in every step. */
    adopt(items) {
      const fresh = items.filter((item) => !history.states.some((state) => state.files.includes(item)));
      if (fresh.length) for (const state of history.states) state.files = withItems(state.files, fresh);
    },
    /** Draws the files again (their numbers changed), the caret kept. */
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
