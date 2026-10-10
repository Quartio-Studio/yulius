// Copy and paste keys in the terminal (SPEC-001 UI-019, F 2026-10-10: "ctrl c ctrl v doesnt work
// in the terminal"). xterm.js turns Ctrl+C into ^C and Ctrl+V into ^V (cancelling the browser's
// paste), so a selection could not be copied with Ctrl+C — it interrupted the program instead — and
// Ctrl+V never pasted. The decision is a pure function of the key event, the platform and whether
// text is selected; terminal-core.js carries it out. Pure module: no DOM access.
//
//   Key                          Selection   Result
//   ⌘C (macOS)                   any         copy
//   Ctrl+C                       yes         copy, then clear the selection (the next Ctrl+C interrupts)
//   Ctrl+C                       no          ^C to the program (interrupt)
//   Ctrl+Shift+C, Ctrl+Insert    any         copy
//   ⌘V (macOS), Ctrl+V (others)  —           paste: the browser's paste event (no permission prompt)
//   Ctrl+V (macOS)               —           paste: read the clipboard (the browser has no paste key)
//   Ctrl+Shift+V, Shift+Insert   —           paste: the browser's paste event
//   anything else                —           the terminal's own handling

/** The letter a key event stands for: `key`, or the physical key on non-Latin layouts. */
function letterOf(event) {
  const key = typeof event.key === 'string' ? event.key : '';
  if (/^[a-z]$/i.test(key)) return key.toLowerCase();
  const code = typeof event.code === 'string' ? event.code : '';
  return /^Key[A-Z]$/.test(code) ? code.slice(3).toLowerCase() : '';
}

/**
 * What a key event means for the clipboard, or null when the terminal handles it as usual.
 * `{ copy: true, clear }`: copy the selection (and clear it after a Ctrl+C);
 * `{ paste: 'native' }`: let the browser's paste command deliver a paste event;
 * `{ paste: 'read' }`: read the clipboard and paste (no browser paste key exists for it).
 * Every event type of a matched key (keydown, keypress, keyup) gets the same answer, so the
 * terminal never sees it; act on keydown only.
 */
export function clipboardKeyAction(event, { mac = false, selection = false } = {}) {
  if (!event || event.altKey) return null; // AltGr (Ctrl+Alt) types characters on Windows layouts
  const ctrl = Boolean(event.ctrlKey);
  const meta = Boolean(event.metaKey);
  const shift = Boolean(event.shiftKey);
  if (event.key === 'Insert') {
    if (ctrl && !shift && !meta) return selection ? { copy: true, clear: false } : null;
    if (shift && !ctrl && !meta) return { paste: 'native' };
    return null;
  }
  const letter = letterOf(event);
  if (letter !== 'c' && letter !== 'v') return null;
  if (mac && meta && !ctrl) {
    if (shift) return null;
    return letter === 'c' ? { copy: true, clear: false } : { paste: 'native' };
  }
  if (!ctrl || meta) return null;
  if (shift) return letter === 'c' ? { copy: true, clear: false } : { paste: mac ? 'read' : 'native' };
  if (letter === 'c') return selection ? { copy: true, clear: true } : null;
  return { paste: mac ? 'read' : 'native' };
}
