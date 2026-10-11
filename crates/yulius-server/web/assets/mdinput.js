// Markdown while typing in a plain textarea (SPEC-001 UI-021, F 2026-10-10): the composer stays a
// fast native text box, with the few keys that make lists and emphasis easy.
//
//   Enter            a new line; in a list (-, *, +, 1., 1), - [ ]) the next item, and on an empty
//                    item the end of the list (an indented one moves out a level first)
//   Tab / Shift+Tab  indent / outdent the list items the selection touches (in a code block: two
//                    spaces); elsewhere, and after Esc, Tab leaves the text box as usual
//   ⌘B / ⌘I          wrap the selection in ** or * (again to unwrap); Ctrl outside macOS
//   ``` + Enter      closes the code block below the caret; inside one, Enter keeps the indent
//   ⌘/Ctrl+Enter     submit (the caller's onSubmit)
//
// IME composition is never interrupted. Edits go through the browser's editing command, so undo
// and redo keep working; where it is missing they fall back to setRangeText and an input event.

import { IS_MAC } from './shortcuts.js';

const LIST = /^(\s*)([-*+]|\d{1,9}[.)])([ \t]+)(\[[ xX]\][ \t]+)?/;
const FENCE = /^\s*(```|~~~)/;

/** The line around `pos`: its start, end and text. */
export function lineAt(value, pos) {
  const start = value.lastIndexOf('\n', pos - 1) + 1;
  const newline = value.indexOf('\n', pos);
  const end = newline === -1 ? value.length : newline;
  return { start, end, text: value.slice(start, end) };
}

/** A list item's parts, or null: indent, marker, the gap after it, a task box and where the text starts. */
export function listItem(text) {
  const match = LIST.exec(text);
  if (!match) return null;
  const [all, indent, marker, gap, box = ''] = match;
  return { indent, marker, gap, box, contentStart: all.length, content: text.slice(all.length) };
}

/** The marker of the item after `item`: the same bullet, the next number, an empty task box. */
export function nextMarker(item) {
  const number = /^(\d+)([.)])$/.exec(item.marker);
  const marker = number ? `${Number(number[1]) + 1}${number[2]}` : item.marker;
  return `${item.indent}${marker}${item.gap}${item.box ? '[ ] ' : ''}`;
}

/** Whether `pos` is inside a fenced code block (an odd number of fence lines before its line). */
export function inFence(value, pos) {
  const before = value.slice(0, lineAt(value, pos).start);
  return before.split('\n').filter((line) => FENCE.test(line)).length % 2 === 1;
}

function fenceCount(value) {
  return value.split('\n').filter((line) => FENCE.test(line)).length;
}

/**
 * What Enter does at a collapsed caret, as an edit `{start, end, text, caret}` on the value, or
 * null for a plain new line.
 */
export function enterEdit(value, pos) {
  const line = lineAt(value, pos);
  const head = line.text.slice(0, pos - line.start);
  const indent = /^\s*/.exec(line.text)[0];
  if (inFence(value, pos)) {
    if (FENCE.test(line.text)) return null; // the closing fence itself
    return { start: pos, end: pos, text: `\n${indent.slice(0, head.length)}`, caret: pos + 1 + Math.min(indent.length, head.length) };
  }
  // An opening fence typed at the end of its line, with no closing fence below: close it.
  if (FENCE.test(line.text) && pos === line.end && fenceCount(value) % 2 === 1) {
    const fence = /```|~~~/.exec(line.text)[0];
    const text = `\n${indent}\n${indent}${fence}`;
    return { start: pos, end: pos, text, caret: pos + 1 + indent.length };
  }
  const item = listItem(line.text);
  if (!item || head.length < item.contentStart) return null;
  if (!item.content.trim()) {
    // An empty item ends the list; nested, it moves out one level first.
    if (item.indent.length > 0) {
      const outer = outdentTo(value, line.start, item.indent.length);
      const text = `${' '.repeat(outer)}${line.text.slice(item.indent.length)}`;
      return { start: line.start, end: line.end, text, caret: line.start + text.length };
    }
    return { start: line.start, end: line.end, text: '', caret: line.start };
  }
  const text = `\n${nextMarker(item)}`;
  return { start: pos, end: pos, text, caret: pos + text.length };
}

/** The indent of the closest list item above `lineStart` that is less indented than `width`. */
function outdentTo(value, lineStart, width) {
  const lines = value.slice(0, Math.max(0, lineStart - 1)).split('\n').reverse();
  for (const text of lines) {
    const item = listItem(text);
    if (item && item.indent.length < width) return item.indent.length;
    if (!item && text.trim()) break;
  }
  return Math.max(0, width - 2);
}

/** How far to indent a list line: to the text of the item above at its level, else two spaces. */
function indentUnit(lines, index) {
  const item = listItem(lines[index]);
  for (let i = index - 1; i >= 0; i -= 1) {
    const above = listItem(lines[i]);
    if (!above) { if (lines[i].trim()) break; continue; }
    if (above.indent.length === item.indent.length) return above.marker.length + above.gap.length;
    if (above.indent.length < item.indent.length) break;
  }
  return 2;
}

/**
 * What Tab (or Shift+Tab with `out`) does to the selection, as `{start, end, text, selStart,
 * selEnd}`, or null when it should move focus as usual (no list line and not in a code block).
 */
export function tabEdit(value, selStart, selEnd, out) {
  const first = lineAt(value, selStart).start;
  // A selection ending at the start of a line does not take that line along.
  const lastPos = selEnd > selStart && value[selEnd - 1] === '\n' ? selEnd - 1 : selEnd;
  const last = lineAt(value, Math.max(selStart, lastPos)).end;
  const lines = value.slice(first, last).split('\n');
  const code = inFence(value, selStart);
  const listed = lines.map((text) => Boolean(listItem(text)));
  if (!code && !listed.some(Boolean)) return null;
  // Every line above the block too, for the indent of the item above the first one.
  const above = value.slice(0, first).split('\n');
  above.pop();
  const all = [...above, ...lines];
  let lineStart = first;
  const deltas = lines.map((text, i) => {
    const start = lineStart;
    lineStart += text.length + 1;
    if (!code && !listed[i]) return 0;
    const current = /^[ \t]*/.exec(text)[0].length;
    if (!out) return listed[i] ? indentUnit(all, above.length + i) : 2;
    if (current === 0) return 0;
    return (listed[i] ? outdentTo(value, start, current) : Math.max(0, current - 2)) - current;
  });
  const text = lines.map((line, i) => (deltas[i] >= 0 ? `${' '.repeat(deltas[i])}${line}` : line.slice(-deltas[i]))).join('\n');
  const total = deltas.reduce((sum, d) => sum + d, 0);
  const caret = Math.max(first, selStart + deltas[0]);
  return { start: first, end: last, text, selStart: caret, selEnd: selEnd === selStart ? caret : Math.max(caret, selEnd + total) };
}

/** Wrapping the selection in `mark` (** or *), or unwrapping it when it already is, as an edit. */
export function wrapEdit(value, selStart, selEnd, mark) {
  const before = value.slice(0, selStart);
  const after = value.slice(selEnd);
  const selected = value.slice(selStart, selEnd);
  const n = mark.length;
  // Runs of stars around the selection and inside it: italic is an odd run (* or ***), bold two or more.
  const runBefore = /\**$/.exec(before)[0].length;
  const runAfter = /^\**/.exec(after)[0].length;
  const innerLead = /^\**/.exec(selected)[0].length;
  const innerTrail = /\**$/.exec(selected)[0].length;
  const marked = (a, b) => (n === 2 ? a >= 2 && b >= 2 : a % 2 === 1 && b % 2 === 1);
  if (marked(runBefore, runAfter)) {
    return { start: selStart - n, end: selEnd + n, text: selected, selStart: selStart - n, selEnd: selEnd - n };
  }
  if (selected.length >= 2 * n && innerLead < selected.length && marked(innerLead, innerTrail)) {
    const inner = selected.slice(n, selected.length - n);
    return { start: selStart, end: selEnd, text: inner, selStart, selEnd: selStart + inner.length };
  }
  return { start: selStart, end: selEnd, text: `${mark}${selected}${mark}`, selStart: selStart + n, selEnd: selEnd + n };
}

/** Applies an edit with the browser's editing command (undo works), else setRangeText. */
function apply(textarea, edit, selStart, selEnd) {
  textarea.focus();
  textarea.setSelectionRange(edit.start, edit.end);
  let done = false;
  try {
    done = edit.text
      ? document.execCommand('insertText', false, edit.text)
      : edit.start === edit.end || document.execCommand('delete', false);
  } catch {
    done = false;
  }
  if (!done || textarea.value.slice(edit.start, edit.start + edit.text.length) !== edit.text) {
    textarea.setRangeText(edit.text, edit.start, edit.end, 'end');
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
  }
  textarea.setSelectionRange(selStart, selEnd);
}

/**
 * Adds the Markdown keys to `textarea`. `onSubmit()` runs on ⌘/Ctrl+Enter. Returns a function that
 * removes them.
 */
export function attachMarkdownInput(textarea, { onSubmit } = {}) {
  // Esc, then Tab, leaves the text box even on a list line (no keyboard trap).
  let escaped = false;
  function onKeyDown(event) {
    // IME composition is never interrupted; a read-only box (while a task starts) is left alone.
    if (event.isComposing || event.keyCode === 229 || event.defaultPrevented || textarea.readOnly || textarea.disabled) return;
    const wasEscaped = escaped;
    escaped = event.key === 'Escape';
    const mod = IS_MAC ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
    const { value, selectionStart: s, selectionEnd: e } = textarea;
    if (event.key === 'Enter') {
      if (event.metaKey || event.ctrlKey) {
        event.preventDefault();
        if (onSubmit) onSubmit();
        return;
      }
      if (event.shiftKey || event.altKey || s !== e) return;
      const edit = enterEdit(value, s);
      if (!edit) return;
      event.preventDefault();
      apply(textarea, edit, edit.caret, edit.caret);
      return;
    }
    if (event.key === 'Tab' && !event.metaKey && !event.ctrlKey && !event.altKey && !wasEscaped) {
      const edit = tabEdit(value, s, e, event.shiftKey);
      // Not a list or code, or nothing to outdent: Tab moves focus, as everywhere.
      if (!edit || edit.text === value.slice(edit.start, edit.end)) return;
      event.preventDefault();
      apply(textarea, edit, edit.selStart, edit.selEnd);
      return;
    }
    if (mod && !event.shiftKey && !event.altKey && (event.key === 'b' || event.key === 'i' || event.key === 'B' || event.key === 'I')) {
      event.preventDefault();
      const edit = wrapEdit(value, s, e, event.key.toLowerCase() === 'b' ? '**' : '*');
      apply(textarea, edit, edit.selStart, edit.selEnd);
    }
  }
  textarea.addEventListener('keydown', onKeyDown);
  return () => textarea.removeEventListener('keydown', onKeyDown);
}
