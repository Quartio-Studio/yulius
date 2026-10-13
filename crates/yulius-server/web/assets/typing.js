// Whether a terminal input frame is a person typing (SPEC-068, ADR-0051), the same rule as the
// server's `is_typing` (crates/yulius-app/src/terminal.rs): a keystroke (text, a control key, an
// Escape key sequence, a bracketed paste) is; what xterm.js sends by itself is not: focus reports,
// SGR and X10 mouse reports and its answers to the program's queries (device attributes, cursor
// position, mode and status reports, OSC and DCS strings). Pure module: no DOM access.

const ESC = '\x1b';

/** Whether `data` (a string as xterm.js emits it) holds a keystroke. */
export function isTyping(data) {
  let i = 0;
  while (i < data.length) {
    if (data[i] !== ESC) return true;
    const kind = data[i + 1];
    if (kind === '[') {
      let j = i + 2;
      const paramsAt = j;
      while (j < data.length && data.charCodeAt(j) >= 0x30 && data.charCodeAt(j) <= 0x3f) j++;
      const params = data.slice(paramsAt, j);
      const midAt = j;
      while (j < data.length && data.charCodeAt(j) >= 0x20 && data.charCodeAt(j) <= 0x2f) j++;
      const intermediates = data.slice(midAt, j);
      // An unfinished sequence is a key (Alt-[), not a report.
      if (j >= data.length) return true;
      const last = data[j];
      let report;
      if (last === 'M' && !params && !intermediates) {
        j += 3; // X10 mouse: button, column and row, one character each
        report = true;
      } else if (last === 'I' || last === 'O') {
        report = !params; // focus in and out
      } else if (last === 'M' || last === 'm' || last === 'c' || last === 'n' || last === 't') {
        report = true; // mouse, device attributes, status answers, window reports
      } else if (last === 'R') {
        report = params.includes(';'); // cursor position
      } else if (last === 'y') {
        report = intermediates === '$'; // mode report
      } else if (last === 'u') {
        report = params.startsWith('?'); // keyboard protocol flags
      } else {
        report = false;
      }
      if (!report) return true;
      i = j + 1;
    } else if (kind === ']' || kind === 'P' || kind === '_' || kind === '^') {
      // OSC, DCS, APC and PM strings end with BEL (OSC only) or ST (ESC \).
      let end = -1;
      for (let k = i + 2; k < data.length; k++) {
        if (kind === ']' && data[k] === '\x07') { end = k + 1; break; }
        if (data[k] === ESC && data[k + 1] === '\\') { end = k + 2; break; }
      }
      if (end < 0) return true; // unterminated: a key (Alt-]), not a string
      i = end;
    } else {
      return true; // Escape alone, SS3 keys, Alt-<key>
    }
  }
  return false;
}
