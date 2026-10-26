// MCP → one server (SPEC-001 UI-038): add or edit with as little typing as possible. Pick a known
// server (its address or command filled in; usually only a token is left), or paste whatever the
// server's docs give you — a URL, a command line, `claude mcp add …` or an `mcpServers` JSON — and
// the fields fill themselves. Remote servers take a URL and a token (sent as Authorization:
// Bearer); local ones a single command line and their environment. Secrets stay write-only: the
// page shows stored key names and replaces values only when asked. After Add, the server opens
// here and is tested at once. Two settings say who gets it (SPEC-332): Available in (every sandbox
// or this one) and For (the whole team or only me); a new server starts on every sandbox and the
// whole team where the person may add that, and both change later (McpServerInput, docs/api.md).

import { enc } from '../api.js';
import { h, icon, iconButton, setChildren, placeholder, loadError, confirmDialog, toast, invalid } from '../dom.js';
import { href, navigate, currentLocation, replaceQuietly, linkTarget, placeFor } from '../router.js';
import { allows } from '../status.js';
import { BRAND } from '../brand.js';
import { notFound } from './common.js';

const KEY_RE = /^[A-Za-z_][A-Za-z0-9_-]*$/;
const NAME_RE = /^[A-Za-z0-9_-]{1,64}$/;

/** Well-known servers: one click fills the connection; what is left is the secret, if any. */
const CATALOG = [
  { id: 'quartio', label: 'Quartio', text: 'Customers, feedback and product docs', logo: '/assets/logos/quartio.svg', onDark: true, transport: 'http', url: 'https://mcp.quartio.ai/mcp', token: 'API token' },
  { id: 'github', label: 'GitHub', text: 'Issues, pull requests and code', iconName: 'github', transport: 'http', url: 'https://api.githubcopilot.com/mcp/', token: 'Personal access token' },
  { id: 'apify', label: 'Apify', text: 'Scrapers and automation actors', logo: '/assets/logos/apify.svg', transport: 'stdio', command: 'npx', args: ['-y', '@apify/actors-mcp-server'], env: ['APIFY_TOKEN'] },
  { id: 'context7', label: 'Context7', text: 'Up-to-date library docs', transport: 'stdio', command: 'npx', args: ['-y', '@upstash/context7-mcp'], env: [] },
  { id: 'stripe', label: 'Stripe', text: 'Payments, customers and invoices', transport: 'http', url: 'https://mcp.stripe.com', token: 'Secret key' },
  { id: 'supabase', label: 'Supabase', text: 'Database, auth and storage', transport: 'stdio', command: 'npx', args: ['-y', '@supabase/mcp-server-supabase@latest'], env: ['SUPABASE_ACCESS_TOKEN'] },
];

/** Split a command line like a shell would (quotes and backslashes), without running anything. */
export function splitCommand(line) {
  const out = [];
  let cur = '';
  let quote = null;
  let any = false;
  for (let i = 0; i < line.length; i += 1) {
    const ch = line[i];
    if (quote) {
      if (ch === quote) quote = null;
      else if (ch === '\\' && quote === '"' && i + 1 < line.length) cur += line[++i];
      else cur += ch;
    } else if (ch === '"' || ch === "'") { quote = ch; any = true; } else if (ch === '\\' && i + 1 < line.length) { cur += line[++i]; any = true; } else if (/\s/.test(ch)) {
      if (cur || any) out.push(cur);
      cur = '';
      any = false;
    } else cur += ch;
  }
  if (cur || any) out.push(cur);
  return out;
}

/** The opposite: one line to show and edit, quoting what needs it. */
export function joinCommand(parts) {
  return parts.map((p) => (/^[A-Za-z0-9_@%+=:,./-]+$/.test(p) ? p : `'${p.replace(/'/g, `'\\''`)}'`)).join(' ');
}

/** A short server name from a URL or a package ("@upstash/context7-mcp" → "context7"). */
export function guessName({ url, command, args = [] }) {
  if (url) {
    try {
      const parts = new URL(url).hostname.split('.').filter((p) => !['mcp', 'api', 'www', 'app', 'dev', 'com', 'ai', 'io', 'org', 'net', 'co'].includes(p));
      if (parts.length) return parts[0].replace(/[^A-Za-z0-9_-]/g, '').slice(0, 64);
    } catch { /* not a URL */ }
  }
  // The package: the first argument that is neither a flag nor a flag's value (uvx --from …, npx -p …).
  const valueFlags = new Set(['--from', '--with', '-p', '--package', '--python', '--registry']);
  const pkg = args.find((a, i) => !a.startsWith('-') && !valueFlags.has(args[i - 1])) || command || '';
  const bare = pkg.replace(/@[^/@]*$/, (v) => (pkg.startsWith('@') && !pkg.slice(1).includes('@') ? v : '')).replace(/@latest$|@[\d.^~]+$/, '');
  const [scope, name] = bare.startsWith('@') ? bare.slice(1).split('/') : [null, bare.split('/').pop()];
  const cleaned = (name || '').replace(/(^|-)mcp(-server)?(?=-|$)/g, '').replace(/(^|-)server(?=-|$)/g, '').replace(/^-+|-+$/g, '');
  const pick = cleaned && !['actors', 'mcp', 'server'].includes(cleaned) ? cleaned : scope || cleaned || name || '';
  return pick.replace(/[^A-Za-z0-9_-]/g, '-').replace(/^-+|-+$/g, '').slice(0, 64);
}

/**
 * Read whatever was pasted: a URL, a command line (leading KEY=value become its environment),
 * `claude mcp add …`, or JSON (`{"mcpServers":{name:{…}}}` or one server's object).
 * Returns { name?, transport, url?, command?, args?, env?, token?, headers? } or null.
 */
export function parseMcpPaste(text) {
  const raw = String(text || '').trim();
  if (!raw) return null;
  if (raw.startsWith('{')) {
    let data;
    try { data = JSON.parse(raw); } catch { return null; }
    let name;
    let cfg = data;
    const servers = data.mcpServers || data.servers || data.mcp_servers;
    if (servers && typeof servers === 'object') [name, cfg] = Object.entries(servers)[0] || [];
    else if (!data.url && !data.command) {
      const entry = Object.entries(data).find(([, v]) => v && typeof v === 'object' && (v.url || v.command));
      if (entry) [name, cfg] = entry;
    }
    if (!cfg || typeof cfg !== 'object') return null;
    const headers = { ...(cfg.headers || {}) };
    let token;
    const auth = Object.keys(headers).find((k) => k.toLowerCase() === 'authorization');
    if (auth && /^Bearer\s+/i.test(headers[auth])) { token = headers[auth].replace(/^Bearer\s+/i, ''); delete headers[auth]; }
    const out = cfg.url
      ? { transport: 'http', url: String(cfg.url), token, headers }
      : { transport: 'stdio', command: String(cfg.command || ''), args: (cfg.args || []).map(String), env: { ...(cfg.env || {}) } };
    out.name = name || guessName(out);
    return out;
  }
  let words = splitCommand(raw);
  if (words[0] === 'claude' && words[1] === 'mcp' && (words[2] === 'add' || words[2] === 'add-json')) {
    if (words[2] === 'add-json') { const parsed = parseMcpPaste(words.slice(4).join(' ')); return parsed ? { ...parsed, name: words[3] || parsed.name } : null; }
    const rest = words.slice(3);
    const env = {};
    const headers = {};
    let transport = null;
    let scope = null;
    const positional = [];
    for (let i = 0; i < rest.length; i += 1) {
      const w = rest[i];
      if (w === '--') { positional.push(...rest.slice(i + 1).map((x) => ({ cmd: x }))); break; }
      if (w === '--transport' || w === '-t') transport = rest[++i];
      else if (w === '--scope' || w === '-s') scope = rest[++i];
      else if (w === '--env' || w === '-e') { const [k, ...v] = String(rest[++i] || '').split('='); if (k) env[k] = v.join('='); } else if (w === '--header' || w === '-H') { const [k, ...v] = String(rest[++i] || '').split(':'); if (k) headers[k.trim()] = v.join(':').trim(); } else positional.push({ word: w });
    }
    const name = positional.find((p) => p.word)?.word;
    const after = positional.filter((p) => p !== positional.find((x) => x.word)).map((p) => p.cmd ?? p.word);
    // Claude Code's user scope is the person's own everywhere, its local scope theirs in this
    // project, its project scope the team's (SPEC-332 MSC-007).
    const audience = scope === 'user' || scope === 'local' ? 'personal' : scope === 'project' ? 'team' : undefined;
    const where = scope === 'local' ? 'sandbox' : undefined;
    if (transport === 'http' || transport === 'sse' || /^https?:\/\//.test(after[0] || '')) {
      const auth = Object.keys(headers).find((k) => k.toLowerCase() === 'authorization');
      const token = auth && /^Bearer\s+/i.test(headers[auth]) ? headers[auth].replace(/^Bearer\s+/i, '') : undefined;
      if (auth && token !== undefined) delete headers[auth];
      return { name: name || guessName({ url: after[0] }), transport: 'http', url: after[0] || '', token, headers, audience, where };
    }
    const [command, ...args] = after;
    return { name: name || guessName({ command, args }), transport: 'stdio', command: command || '', args, env, audience, where };
  }
  if (/^https?:\/\/\S+$/.test(raw)) return { name: guessName({ url: raw }), transport: 'http', url: raw };
  const env = {};
  while (words.length && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[0])) { const [k, ...v] = words.shift().split('='); env[k] = v.join('='); }
  if (!words.length) return null;
  const [command, ...args] = words;
  return { name: guessName({ command, args }), transport: 'stdio', command, args, env };
}

/** Mount the page: `id` is a server id, or 'new'. */
export function mountMcpEditor(ctx, sandbox, id) {
  const base = `/s/${encodeURIComponent(sandbox.slug)}/mcp`;
  const creating = id === 'new';
  const write = allows(sandbox.access, 'write');
  const admin = ctx.isInstanceAdmin();
  const query = currentLocation().query;
  let server = null;
  let servers = [];
  let canEdit = write;
  let alive = true;
  let saved = '';
  let picked = null; // catalog id
  // The form: env/header rows hold [key, value]; values are only what was typed (write-only).
  // A new server starts on every sandbox and the whole team (SPEC-332 MSC-002), which only an
  // administrator may add; anyone else starts on this sandbox.
  const state = { name: '', transport: 'http', url: '', line: '', token: '', replaceToken: true, env: [], replaceEnv: true, headers: [], scope: admin && query.get('scope') !== 'sandbox' ? 'instance' : 'sandbox', audience: 'team' };
  // Every sandbox for the whole team is an administrator's; personal servers and this sandbox's
  // team servers are anyone's who may write here.
  const allowed = (scope, audience) => canEdit && (audience === 'personal' || scope === 'sandbox' || admin);
  const live = h('div', { class: 'sr-only', role: 'status' });

  const nameInput = h('input', { class: 'wf-name', placeholder: 'server-name', 'aria-label': 'Server name', maxlength: 64, autocomplete: 'off', spellcheck: 'false', onInput: (e) => { state.name = e.target.value; changed(); } });
  const save = h('button', { type: 'button', class: 'btn btn-primary', onClick: () => doSave() }, creating ? 'Add server' : 'Save');
  const del = creating ? null : h('button', { type: 'button', class: 'btn btn-quiet-danger', onClick: () => doDelete() }, icon('trash', { size: 14 }), 'Delete');
  const note = h('div');
  const body = h('div', { class: 'mc-body' }, placeholder('Loading the MCP server…', 'loading', 'form'));
  const head = h('header', { class: 'gt-head wf-head' },
    h('a', { class: 'gt-back', href: href(base), dataset: { route: base, restore: 'page' } }, icon('back', { size: 14 }), 'MCP'),
    h('div', { class: 'gt-title-row' }, h('div', { class: 'wf-titles' }, nameInput, h('span', { class: 'muted small' }, 'Claude Code sees its tools under this name.')), del, save),
    note);
  const el = h('div', { class: 'page mc-page' }, h('div', { class: 'page-inner mc-inner' }, h('h1', { class: 'sr-only' }, creating ? 'Add MCP server' : 'Edit MCP server'), head, body, live));

  const snapshot = () => JSON.stringify(state);
  const dirty = () => snapshot() !== saved;
  function changed() { save.disabled = !canEdit || (!creating && !dirty()); }

  // ---- fill from the catalog or a paste
  function fill(spec, from) {
    state.transport = spec.transport;
    if (spec.audience) { state.audience = spec.audience; if (!allowed(state.scope, state.audience)) state.scope = 'sandbox'; }
    if (spec.where) state.scope = spec.where;
    if (spec.name) { state.name = spec.name; nameInput.value = spec.name; }
    if (spec.transport === 'http') {
      state.url = spec.url || '';
      if (spec.token !== undefined) { state.token = spec.token; state.replaceToken = true; }
      state.headers = Object.entries(spec.headers || {});
    } else {
      state.line = joinCommand([spec.command, ...(spec.args || [])].filter((x) => x !== undefined && x !== ''));
      const env = Array.isArray(spec.env) ? spec.env.map((k) => [k, '']) : Object.entries(spec.env || {});
      if (env.length || creating) { state.env = env; state.replaceEnv = true; }
    }
    render();
    live.textContent = `Filled from ${from}`;
    const firstEmpty = el.querySelector('.mc-secret input[type=password]:placeholder-shown, .mc-conn input:placeholder-shown');
    if (firstEmpty && from !== 'the paste') firstEmpty.focus();
  }

  function catalogSection() {
    const taken = new Set(servers.map((s) => s.name.toLowerCase()));
    return section('Pick a server', 'or paste below', h('div', { class: 'mc-catalog', role: 'radiogroup', 'aria-label': 'Known servers' }, CATALOG.map((c) => {
      const added = taken.has(c.id);
      const mark = c.logo ? h('img', { class: c.onDark ? 'logo-on-dark' : null, src: c.logo, alt: '', width: 20, height: 20 }) : c.iconName ? icon(c.iconName, { size: 18 }) : h('span', { class: 'mc-mono', 'aria-hidden': 'true' }, c.label[0]);
      return h('button', { type: 'button', role: 'radio', 'aria-checked': String(picked === c.id), class: ['wf-template', 'mc-pick', picked === c.id && 'on'], disabled: added || !canEdit, title: added ? `${c.label} is already attached` : null,
        onClick: () => { picked = c.id; fill({ name: c.id, transport: c.transport, url: c.url, command: c.command, args: c.args, env: c.env, token: c.token ? '' : undefined }, c.label); } },
      h('span', { class: 'wf-template-icon mc-logo' }, mark),
      h('span', { class: 'wf-template-name' }, c.label, added ? h('span', { class: 'mc-added' }, icon('check', { size: 12 }), 'Added') : null),
      h('span', { class: 'muted xs' }, c.text));
    })));
  }

  function pasteSection() {
    const box = h('textarea', { class: 'textarea mono mc-paste', rows: 1, placeholder: 'Paste a URL, a command, “claude mcp add …” or the JSON from the server’s docs', 'aria-label': 'Paste a server’s URL, command or JSON', spellcheck: 'false' });
    const result = h('span', { class: 'muted small', role: 'status' });
    const apply = () => {
      const spec = parseMcpPaste(box.value);
      if (!spec) { result.textContent = box.value.trim() ? 'Couldn’t read that: paste a URL, a command line or JSON.' : ''; return; }
      picked = null;
      box.value = '';
      fill(spec, 'the paste');
      el.querySelector('.mc-paste-result').textContent = `Filled in from what you pasted${spec.token || Object.values(spec.env || {}).some(Boolean) ? ', secrets included' : ''}. Check it, then ${creating ? 'add' : 'save'}.`;
    };
    box.addEventListener('paste', () => setTimeout(apply, 0));
    box.addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); apply(); } });
    result.classList.add('mc-paste-result');
    return h('div', { class: 'mc-paste-wrap' }, h('div', { class: 'mc-paste-field' }, icon('download', { size: 16 }), box), result);
  }

  // ---- connection
  function connectionSection() {
    const option = (value, label, iconName) => h('label', null,
      h('input', { type: 'radio', name: 'mc-transport', value, checked: state.transport === value, disabled: !canEdit, onChange: () => { state.transport = value; render(); } }),
      icon(iconName, { size: 14 }), label);
    const kind = h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Connection' }, option('http', 'Remote (URL)', 'globe'), option('stdio', 'Local command', 'terminal'));
    const input = state.transport === 'http'
      ? h('input', { class: 'input mono', type: 'url', value: state.url, readOnly: !canEdit, placeholder: 'https://mcp.example.com/mcp', 'aria-label': 'Server URL', spellcheck: 'false', onInput: (e) => { state.url = e.target.value; changed(); } })
      : h('input', { class: 'input mono', value: state.line, readOnly: !canEdit, placeholder: 'npx -y @scope/some-mcp-server', 'aria-label': 'Command line', spellcheck: 'false', onInput: (e) => { state.line = e.target.value; changed(); } });
    return section('Connection', null, h('div', { class: 'mc-conn' }, kind, input,
      h('span', { class: 'muted xs' }, state.transport === 'http' ? 'The address of a Streamable HTTP server.' : 'Runs inside the sandbox for each session, e.g. npx or uvx with its package.')));
  }

  // ---- secrets (write-only)
  function kvRows(rows, keyLabel, valueLabel, onChange) {
    const host = h('div', { class: 'kv-rows' });
    const draw = () => setChildren(host, rows.map((row, i) => h('div', { class: 'kv-row' },
      h('input', { class: 'input mono', value: row[0], placeholder: 'NAME', readOnly: !canEdit, 'aria-label': `${keyLabel} ${i + 1}`, spellcheck: 'false', onInput: (e) => { row[0] = e.target.value; onChange(); } }),
      h('input', { class: 'input mono', type: 'password', value: row[1], placeholder: row[0] ? `value of ${row[0]}` : 'value', autocomplete: 'off', readOnly: !canEdit, 'aria-label': `${valueLabel} ${row[0] || i + 1}`, onInput: (e) => { row[1] = e.target.value; onChange(); } }),
      canEdit ? iconButton('x', `Remove ${row[0] || 'row'}`, () => { rows.splice(i, 1); draw(); onChange(); }) : null)));
    draw();
    return h('div', { class: 'stack-sm' }, host, canEdit ? h('div', null, h('button', { type: 'button', class: 'btn btn-sm', onClick: () => { rows.push(['', '']); draw(); host.querySelector('.kv-row:last-child input')?.focus(); onChange(); } }, icon('plus', { size: 12 }), 'Add a variable')) : null);
  }

  function storedLine(keys, replaceLabel, onReplace) {
    return h('div', { class: 'mc-stored' }, icon('lock', { size: 14 }), h('span', null, 'Stored: '), keys.map((k) => h('span', { class: 'mc-key mono' }, `${k} ••••`)),
      canEdit ? h('button', { type: 'button', class: 'link-btn', onClick: onReplace }, replaceLabel) : null);
  }

  function secretsSection() {
    if (state.transport === 'http') {
      const storedToken = !creating && server.transport === 'http' && (server.header_keys || []).some((k) => k.toLowerCase() === 'authorization');
      const otherStored = !creating && server.transport === 'http' ? (server.header_keys || []).filter((k) => k.toLowerCase() !== 'authorization') : [];
      const tokenField = storedToken && !state.replaceToken
        ? storedLine(['Token'], 'Replace the token', () => { state.replaceToken = true; render(); el.querySelector('.mc-token')?.focus(); })
        : h('input', { class: 'input mono mc-token', type: 'password', value: state.token, autocomplete: 'off', readOnly: !canEdit, placeholder: (CATALOG.find((c) => c.id === picked) || {}).token || 'Token (if the server needs one)', 'aria-label': 'Access token', onInput: (e) => { state.token = e.target.value; changed(); } });
      return section('Access', 'write-only', h('div', { class: 'mc-secret stack-sm' }, tokenField,
        h('span', { class: 'muted xs' }, 'Sent as Authorization: Bearer …, stored encrypted and never shown again.'),
        h('details', { class: 'mc-more', open: state.headers.length > 0 || otherStored.length > 0 },
          h('summary', null, 'Other headers'),
          otherStored.length && !state.headers.length ? storedLine(otherStored, 'Replace', () => { state.headers = otherStored.map((k) => [k, '']); render(); }) : null,
          kvRows(state.headers, 'Header', 'Value of header', changed),
          h('p', { class: 'muted xs' }, 'Used when testing; sessions get the token only.'))));
    }
    const storedKeys = !creating && server.transport === 'stdio' ? server.env_keys || [] : [];
    if (storedKeys.length && !state.replaceEnv) {
      return section('Environment', 'write-only', h('div', { class: 'mc-secret stack-sm' },
        storedLine(storedKeys, 'Replace the values', () => { state.replaceEnv = true; state.env = storedKeys.map((k) => [k, '']); render(); el.querySelector('.mc-secret input[type=password]')?.focus(); })));
    }
    return section('Environment', 'write-only', h('div', { class: 'mc-secret stack-sm' },
      state.env.length || canEdit ? kvRows(state.env, 'Variable', 'Value of', changed) : h('p', { class: 'muted small' }, 'None.'),
      storedKeys.length ? h('p', { class: 'muted xs' }, 'Saving replaces every stored value: enter each one again.') : h('p', { class: 'muted xs' }, 'API keys and tokens the command needs; stored encrypted and never shown again.')));
  }

  function scopeSection() {
    const ADMIN_ONLY = 'Every sandbox for the whole team is set by instance administrators: choose Only me to have it in every sandbox';
    const option = (group, value, label, iconName, ok, why) => h('label', { title: ok ? null : why },
      h('input', { type: 'radio', name: `mc-${group}`, value, checked: state[group] === value, disabled: !ok, onChange: () => {
        state[group] = value;
        // A combination the person may not set gives way to the one they may (MSC-003).
        if (!allowed(state.scope, state.audience)) state.scope = 'sandbox';
        render();
      } }), icon(iconName, { size: 14 }), label);
    const who = state.audience === 'personal'
      ? 'Only your own tasks and your Superagent get it — never a teammate’s task or an automation’s.'
      : state.scope === 'instance' ? 'Every task in every sandbox gets it.' : `Every task in ${sandbox.name} gets it.`;
    return section('Who gets it', null, h('div', { class: 'wf-scope stack-sm' },
      h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Available in' },
        option('scope', 'instance', 'Every sandbox', 'globe', allowed('instance', state.audience), ADMIN_ONLY),
        option('scope', 'sandbox', 'This sandbox', 'folder', canEdit, '')),
      h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'For' },
        option('audience', 'team', 'Whole team', 'users', canEdit, ''),
        option('audience', 'personal', 'Only me', 'user', canEdit, '')),
      h('span', { class: 'muted xs mc-who' }, who),
      h('span', { class: 'muted xs' }, 'Of servers with the same name the most specific wins: this sandbox over every sandbox, yours over the team’s.')));
  }

  // ---- test
  const testResult = h('div', { class: 'mc-test-result', role: 'status' });
  async function runTest() {
    const button = el.querySelector('.mc-test');
    if (!server || (button && button.classList.contains('busy'))) return;
    button?.classList.add('busy');
    setChildren(testResult, h('span', { class: 'muted small' }, 'Connecting…'));
    try {
      const r = await ctx.api.post(`/api/mcp/${enc(server.id)}/test`);
      if (!alive) return;
      setChildren(testResult, r.ok
        ? [h('p', { class: 'mc-ok small' }, icon('check', { size: 14 }), `Connected · ${r.tools.length} ${r.tools.length === 1 ? 'tool' : 'tools'}`), h('div', { class: 'wf-chips' }, r.tools.map((t) => h('span', { class: 'mc-key mono' }, t)))]
        : h('p', { class: 'mc-fail small' }, icon('warn', { size: 14 }), `Couldn’t connect: ${r.error || 'no answer'}`));
    } catch (error) {
      if (alive) setChildren(testResult, h('p', { class: 'mc-fail small' }, icon('warn', { size: 14 }), `Couldn’t test: ${error.message}`));
    } finally {
      button?.classList.remove('busy');
    }
  }
  // A server from the repository's .mcp.json (agents can write it): its local command is never
  // tested on the host, nor its private address, until a person changes them (SPEC-218, #654).
  const imported = () => server && server.origin === 'mcp_json';
  const refused = () => imported() && server.transport === 'stdio';
  function importedNote() {
    return h('p', { class: 'banner banner-warn small' }, icon('warn', { size: 14 }), h('span', { class: 'banner-text' },
      server.transport === 'stdio'
        ? 'This server came from the repository’s .mcp.json, which agents can write. Its command is not tested on the server until you have checked the whole command line, changed it and entered its environment again.'
        : 'This server came from the repository’s .mcp.json, which agents can write. A private or local address is not tested until you change its URL.'));
  }
  function testSection() {
    const contained = Boolean(ctx.store.state.me && ctx.store.state.me.instance && ctx.store.state.me.instance.contained);
    const allowed = canEdit && !refused() && (server.transport !== 'stdio' || !contained || admin);
    const why = refused() ? 'From the repository’s .mcp.json: tested once you have checked and changed its command' : 'With isolation on, local-command servers are tested by instance administrators';
    return section('Test', null, h('div', { class: 'stack-sm' },
      imported() ? importedNote() : null,
      h('div', null, h('button', { type: 'button', class: 'btn mc-test', disabled: !allowed, title: allowed ? 'Connect and list its tools' : why, onClick: runTest }, icon('plug', { size: 14 }), 'Test connection')),
      testResult));
  }

  function section(title, hint, content) {
    return h('section', { class: 'wf-section' }, h('h2', { class: 'wf-section-title' }, title, hint ? h('span', { class: 'muted small' }, hint) : null), content);
  }

  function render() {
    setChildren(body, h('div', { class: 'wf-main' },
      creating && canEdit ? catalogSection() : null,
      canEdit ? pasteSection() : null,
      connectionSection(),
      secretsSection(),
      scopeSection(),
      creating ? null : testSection()));
    changed();
  }

  // ---- save, remove, leave
  function payload() {
    const out = { name: state.name.trim(), transport: state.transport, enabled: server ? server.enabled : true };
    if (state.transport === 'http') {
      out.url = state.url.trim() || null;
      out.command = null;
      out.args = [];
      const headers = {};
      for (const [k, v] of state.headers) {
        if (!k.trim() && !v) continue;
        if (!KEY_RE.test(k.trim())) throw new Error(`“${k}” is not a valid header name.`);
        headers[k.trim()] = v;
      }
      // Headers are sent only when something was typed, and a sent map replaces every stored
      // header (write-only): so the other half must be entered again too, never dropped silently.
      const stored = !creating && server.transport === 'http' ? server.header_keys || [] : [];
      const storedToken = stored.some((k) => k.toLowerCase() === 'authorization');
      const storedOther = stored.filter((k) => k.toLowerCase() !== 'authorization');
      const token = state.token.trim();
      if (creating || token || Object.keys(headers).length) {
        if (!creating && storedToken && !token) throw new Error('Saving other headers replaces the stored token too: replace the token as well.');
        if (!creating && storedOther.length && !Object.keys(headers).length) throw new Error('A new token replaces the other stored headers too: enter them again under Other headers.');
        if (token) headers.Authorization = `Bearer ${token}`;
        out.headers = headers;
      }
    } else {
      const words = splitCommand(state.line.trim());
      out.command = words[0] || null;
      out.args = words.slice(1);
      out.url = null;
      if (state.replaceEnv) {
        const env = {};
        for (const [k, v] of state.env) {
          if (!k.trim() && !v) continue;
          if (!KEY_RE.test(k.trim())) throw new Error(`“${k}” is not a valid variable name.`);
          if (!v) throw new Error(`Enter a value for ${k.trim()}.`);
          env[k.trim()] = v;
        }
        out.env = env;
      }
    }
    out.scope = state.scope;
    out.audience = state.audience;
    // A server moved into one sandbox belongs to the one this page is about.
    if (!creating) out.sandbox = sandbox.id;
    return out;
  }

  async function doSave() {
    if (!canEdit || save.classList.contains('busy')) return;
    const name = state.name.trim();
    if (!NAME_RE.test(name)) { invalid(nameInput, 'Use 1–64 letters, digits, “-” or “_”.'); return; }
    if (name.toLowerCase() === 'yulius') { invalid(nameInput, `“yulius” is reserved for the built-in ${BRAND.name} tool server.`); return; }
    if (state.transport === 'http' && !/^https?:\/\/\S+$/.test(state.url.trim())) { invalid(el.querySelector('.mc-conn input.input'), 'Enter the server’s address, starting with https://'); return; }
    if (state.transport === 'stdio' && !state.line.trim()) { invalid(el.querySelector('.mc-conn input.input'), 'Enter the command that starts the server.'); return; }
    let out;
    // Said at the stored-secret fields it is about, not in a toast.
    try { out = payload(); } catch (error) { invalid(el.querySelector('.mc-secret input') || nameInput, error.message); return; }
    save.classList.add('busy');
    try {
      const result = creating ? await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/mcp`, out) : await ctx.api.put(`/api/mcp/${enc(id)}`, out);
      saved = snapshot();
      toast(creating ? `MCP server ${name} added.` : `MCP server ${name} saved.`, { kind: 'ok' });
      if (creating) navigate(`${base}/${encodeURIComponent(result.id)}?test=1`);
      else { server = result; canEdit = write && (result.scope !== 'instance' || admin || result.audience === 'personal'); Object.assign(state, { scope: result.scope, audience: result.audience || 'team' }); state.token = ''; state.replaceToken = false; state.replaceEnv = !(result.env_keys || []).length; state.env = []; state.headers = []; saved = snapshot(); render(); }
    } catch (error) {
      toast(`Couldn’t ${creating ? 'add' : 'save'}: ${error.message}`, { kind: 'error' });
    } finally {
      save.classList.remove('busy');
    }
  }

  async function doDelete() {
    if (!(await confirmDialog({ title: `Delete MCP server “${server.name}”?`, message: 'New sessions no longer get this server; its stored secrets are deleted.', confirmLabel: 'Delete', danger: true }))) return;
    try {
      await ctx.api.del(`/api/mcp/${enc(id)}`);
      saved = snapshot();
      toast(`MCP server ${server.name} deleted.`, { kind: 'ok' });
      navigate(placeFor(base, 'page'));
    } catch (error) {
      toast(`Couldn’t delete: ${error.message}`, { kind: 'error' });
    }
  }

  const onBeforeUnload = (event) => { if (dirty()) { event.preventDefault(); event.returnValue = ''; } };
  const onLink = async (event) => {
    const link = event.target.closest && event.target.closest('a[data-route]');
    if (!link || !dirty() || event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    if (await confirmDialog({ title: 'Leave without saving?', message: 'Your changes to this server are not saved.', confirmLabel: 'Leave', danger: true })) { saved = snapshot(); navigate(linkTarget(link)); }
  };
  window.addEventListener('beforeunload', onBeforeUnload);
  document.addEventListener('click', onLink, true);
  el.addEventListener('keydown', (event) => {
    if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === 's') { event.preventDefault(); doSave(); }
  });

  async function load() {
    try {
      const list = await ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/mcp`);
      if (!alive) return;
      servers = Array.isArray(list) ? list : [];
      if (!creating) {
        server = servers.find((s) => s.id === id);
        // Nothing to edit, so nothing to guard: the way back leaves without asking.
        if (!server) { saved = snapshot(); setChildren(el, notFound({ title: 'MCP server not found', text: `There is no MCP server “${id}” for ${sandbox.name}. It may have been deleted.`, back: ['MCP', base] })); return; }
        canEdit = write && (server.scope !== 'instance' || admin || server.audience === 'personal');
        Object.assign(state, {
          name: server.name, transport: server.transport, url: server.url || '', line: joinCommand([server.command, ...(server.args || [])].filter((x) => x != null)),
          token: '', replaceToken: !(server.header_keys || []).some((k) => k.toLowerCase() === 'authorization'), env: [], replaceEnv: !(server.env_keys || []).length, headers: [], scope: server.scope, audience: server.audience || 'team',
        });
        nameInput.value = server.name;
      }
      nameInput.readOnly = !canEdit;
      save.hidden = !canEdit;
      if (del) del.hidden = !canEdit;
      setChildren(note, canEdit ? null : h('p', { class: 'banner small' }, icon('lock', { size: 14 }), h('span', { class: 'banner-text' }, !write ? 'Read-only: changing MCP servers needs write access to this sandbox.' : 'Read-only: servers for every sandbox are changed by instance administrators.')));
      saved = snapshot();
      render();
      if (creating) el.querySelector('.mc-paste')?.focus({ preventScroll: true });
      if (!creating && query.get('test') === '1') { replaceQuietly(`${base}/${encodeURIComponent(id)}`); if (!refused()) runTest(); }
    } catch (error) {
      if (alive) setChildren(body, loadError(`Couldn’t load the servers: ${error.message}`, load));
    }
  }
  load();

  return {
    el,
    destroy() { alive = false; window.removeEventListener('beforeunload', onBeforeUnload); document.removeEventListener('click', onLink, true); },
  };
}
