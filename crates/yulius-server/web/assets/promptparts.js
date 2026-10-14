// A prompt's text and its attached files, in order (SPEC-124, ADR-0101): the composer writes each
// file where it sits in the text as a quoted mention `@'/path'`; an older prompt has its files
// after the text, a line `@/path` or `@'/path'` each. This reads both as the server does
// (`Request::parse` in yulius-domain): the mock, the composer's drafts and the original prompt
// view use it.

/** The mention the composer writes for a file: POSIX single quotes, always. */
export function mentionOf(path) {
  return `@'${String(path).replace(/'/g, "'\\''")}'`;
}

/** A path a mention can name: absolute, with no control character. */
const validPath = (path) => path.length > 1 && path.startsWith('/') && !/[\u0000-\u001f\u007f-\u009f]/.test(path);

/** The path of a quoted mention after its opening quote, with the characters it takes, or null. */
function quotedPath(quoted) {
  let path = '';
  let rest = quoted;
  for (;;) {
    const end = rest.indexOf("'");
    if (end === -1) return null;
    path += rest.slice(0, end);
    rest = rest.slice(end + 1);
    if (!rest.startsWith("\\''")) break;
    path += "'";
    rest = rest.slice(3);
  }
  return validPath(path) ? { path, used: quoted.length - rest.length } : null;
}

/** The path of a line that is only a mention (`@/a.png`, `@'/a b.png'`), or null. */
function linePath(line) {
  if (!line.startsWith('@')) return null;
  const rest = line.slice(1);
  if (rest.startsWith("'")) {
    const quoted = quotedPath(rest.slice(1));
    return quoted && quoted.used === rest.length - 1 ? quoted.path : null;
  }
  return /\s/.test(rest) || !validPath(rest) ? null : rest;
}

const withoutControls = (text) => text.replace(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g, '');

/**
 * `{ text, paths, parts }`: `paths` the attached files in order, `parts` the prompt in order as
 * `{ text }` and `{ file: n }` (an index into `paths`), blank text at either end left out; `text`
 * the text without its files.
 */
export function parsePrompt(prompt) {
  const parts = [];
  const paths = [];
  const kept = [];
  const pushText = (text) => {
    const clean = withoutControls(text);
    if (!clean) return;
    const last = parts[parts.length - 1];
    if (last && 'text' in last) last.text += clean;
    else parts.push({ text: clean });
  };
  const file = (path) => { parts.push({ file: paths.length }); paths.push(path); };
  String(prompt ?? '').split(/\r?\n/).forEach((line, index) => {
    if (index > 0) pushText('\n');
    const whole = linePath(line.trim());
    if (whole) { file(whole); return; }
    let out = '';
    let afterFile = false;
    const keep = (piece) => {
      const doubled = afterFile && /\s$/.test(out) && /^\s/.test(piece);
      out += doubled ? piece.slice(1) : piece;
    };
    let rest = line;
    for (let at = rest.indexOf("@'"); at !== -1; at = rest.indexOf("@'")) {
      const quoted = quotedPath(rest.slice(at + 2));
      if (quoted) {
        pushText(rest.slice(0, at));
        keep(rest.slice(0, at));
        file(quoted.path);
        afterFile = true;
        rest = rest.slice(at + 2 + quoted.used);
      } else {
        pushText(rest.slice(0, at + 2));
        keep(rest.slice(0, at + 2));
        afterFile = false;
        rest = rest.slice(at + 2);
      }
    }
    pushText(rest);
    keep(rest);
    kept.push(out);
  });
  if (parts.length && 'text' in parts[0]) parts[0].text = parts[0].text.trimStart();
  const last = parts[parts.length - 1];
  if (last && 'text' in last) last.text = last.text.trimEnd();
  return {
    text: withoutControls(kept.join('\n')).trim(),
    paths,
    parts: parts.filter((part) => !('text' in part) || part.text),
  };
}

/** How many of `parts` sit inside the text: the files after the last text that is not blank don't. */
export function inlineCount(parts) {
  for (let i = parts.length - 1; i >= 0; i -= 1) if ('text' in parts[i] && parts[i].text.trim()) return i + 1;
  return 0;
}

/**
 * `text` as written, split at its quoted mentions: `[{ text } | { path, mention }]`, nothing
 * trimmed or dropped (`mention` is the mention as it was written).
 */
export function splitMentions(text) {
  const out = [];
  let rest = String(text ?? '');
  let buffer = '';
  for (let at = rest.indexOf("@'"); at !== -1; at = rest.indexOf("@'")) {
    const quoted = quotedPath(rest.slice(at + 2));
    if (quoted) {
      buffer += rest.slice(0, at);
      if (buffer) out.push({ text: buffer });
      buffer = '';
      out.push({ path: quoted.path, mention: rest.slice(at, at + 2 + quoted.used) });
      rest = rest.slice(at + 2 + quoted.used);
    } else {
      buffer += rest.slice(0, at + 2);
      rest = rest.slice(at + 2);
    }
  }
  buffer += rest;
  if (buffer) out.push({ text: buffer });
  return out;
}
