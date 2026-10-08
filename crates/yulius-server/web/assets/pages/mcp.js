// MCP page (MCP-001..003): sandbox servers plus inherited instance servers; add, edit,
// test. Secrets are write-only: the UI shows key names only and never reads values back.

import { enc } from '../api.js';
import { h, icon, badge, openModal, field, toast, confirmDialog, errorToast, toggle, setChildren, iconButton } from '../dom.js';
import { allows } from '../status.js';
import { pageFrame, loader } from './common.js';

const KEY_RE = /^[A-Za-z_][A-Za-z0-9_-]*$/;
/** Server names: 1–64 letters, digits, `-` or `_`; `yulius` is reserved. */
const NAME_RE = /^[A-Za-z0-9_-]{1,64}$/;

/** Logos of well-known servers (assets/logos, see its README), matched by name, URL or package. */
const LOGOS = [
  { src: '/assets/logos/quartio.svg', label: 'Quartio', test: /quartio/i },
  { src: '/assets/logos/apify.svg', label: 'Apify', test: /apify/i },
];

/** The server's logo when it is a known service, else an icon for its transport. */
function serverLogo(server) {
  const haystack = [server.name, server.url, server.command, ...(server.args || [])].filter(Boolean).join(' ');
  const known = LOGOS.find((logo) => logo.test.test(haystack));
  return h('span', { class: 'server-logo', title: known ? known.label : server.transport === 'http' ? 'HTTP server' : 'Local command (stdio)' },
    known ? h('img', { src: known.src, alt: '', width: 22, height: 22 }) : icon(server.transport === 'http' ? 'globe' : 'terminal', { size: 18 }));
}

/** A badge with an icon in front. */
function iconBadge(iconName, text, tone, title) {
  const el = badge(text, tone, title);
  el.prepend(icon(iconName, { size: 11 }));
  return el;
}

/** Mount the MCP page; ?scope=instance pre-filters (admin "Instance MCP" entry). */
export function mountMcpPage(ctx, sandbox, route) {
  const write = allows(sandbox.access, 'write');
  const instanceAdmin = ctx.isInstanceAdmin();
  const scopeFilter = h('select', { class: 'select', 'aria-label': 'Scope filter' }, h('option', { value: '' }, 'All servers'), h('option', { value: 'sandbox' }, 'This sandbox'), h('option', { value: 'instance' }, 'Instance (inherited)'));
  scopeFilter.value = route.query.get('scope') || '';
  const frame = pageFrame({
    title: 'MCP',
    sub: 'attached once, available to every Claude Code session',
    actions: [h('button', { type: 'button', class: 'btn btn-primary btn-sm', disabled: !write, title: write ? null : 'Requires write access', onClick: () => edit(null) }, 'Add server')],
  });
  const host = h('div');
  frame.body.append(h('div', { class: 'toolbar' }, scopeFilter, h('span', { class: 'muted small' }, 'The Yulius tool server is always attached and not listed here.')), host);
  let servers = [];

  const canManage = (server) => write && (server.scope !== 'instance' || instanceAdmin);
  // Tests follow the write rule; stdio tests run outside the containment, so a contained
  // instance lets only instance administrators run them.
  const contained = Boolean(ctx.store.state.me && ctx.store.state.me.instance && ctx.store.state.me.instance.contained);
  const canTest = (server) => canManage(server) && (server.transport !== 'stdio' || !contained || instanceAdmin);

  function render() {
    const shown = servers.filter((s) => !scopeFilter.value || s.scope === scopeFilter.value);
    if (shown.length === 0) return h('div', { class: 'card placeholder' }, servers.length ? 'No server in this scope.' : 'No MCP servers yet. Add e.g. the Quartio MCP (HTTP) or the Apify MCP (stdio).');
    return h('div', { class: 'card list' }, shown.map((server) => {
      const testResult = h('div', { class: 'small' });
      return h('div', { class: 'list-item', style: { alignItems: 'flex-start' } },
        toggle({ checked: server.enabled, disabled: !canManage(server), label: `${server.enabled ? 'Disable' : 'Enable'} ${server.name}`, onChange: async (checked, input) => {
          try {
            await ctx.api.put(`/api/mcp/${enc(server.id)}`, { ...baseBody(server), enabled: checked });
            server.enabled = checked;
          } catch (error) {
            input.checked = !checked;
            errorToast(error, 'Could not update server');
          }
        } }),
        serverLogo(server),
        h('div', { class: 'list-main' },
          h('div', { class: 'list-title' }, server.name,
            server.scope === 'instance' ? iconBadge('globe', 'instance', 'violet', 'Attached to every sandbox of this instance') : iconBadge('folder', 'sandbox', '', 'Attached to this sandbox only'),
            iconBadge(server.transport === 'http' ? 'external' : 'terminal', server.transport, 'accent', server.transport === 'http' ? 'Remote server over HTTP' : 'Local command over stdio')),
          h('div', { class: 'list-sub mono' }, server.transport === 'http' ? server.url || '' : [server.command, ...(server.args || [])].filter(Boolean).join(' ')),
          (server.env_keys || []).length || (server.header_keys || []).length
            ? h('div', { class: 'chips', style: { marginTop: '2px' } },
              (server.env_keys || []).map((k) => iconBadge('lock', `env ${k} ••••`, '', 'Secret value is stored encrypted and never shown')),
              (server.header_keys || []).map((k) => iconBadge('lock', `header ${k} ••••`, '', 'Secret value is stored encrypted and never shown')))
            : null,
          testResult),
        h('div', { class: 'list-actions' },
          h('button', { type: 'button', class: 'btn btn-sm', disabled: !canTest(server), title: canTest(server) ? 'Connect and list tools (HTTP ≤ 5 s, stdio ≤ 10 s)' : server.scope === 'instance' && !instanceAdmin ? 'Instance servers are tested by instance administrators' : server.transport === 'stdio' && contained ? 'On a contained instance stdio tests need an instance administrator' : 'Requires write access', onClick: async (event) => {
            const button = event.currentTarget;
            button.classList.add('busy');
            try {
              const result = await ctx.api.post(`/api/mcp/${enc(server.id)}/test`);
              setChildren(testResult, result.ok
                ? h('span', { class: 'row-wrap' }, badge('ok', 'ok'), h('span', { class: 'muted' }, `${(result.tools || []).length} tools: ${(result.tools || []).slice(0, 12).join(', ')}${(result.tools || []).length > 12 ? '…' : ''}`))
                : h('span', { class: 'row-wrap' }, badge('failed', 'danger'), h('span', { class: 'field-error' }, result.error || 'unknown error')));
            } catch (error) {
              errorToast(error, 'Test failed');
            } finally {
              button.classList.remove('busy');
            }
          } }, 'Test'),
          h('button', { type: 'button', class: 'btn btn-sm', disabled: !canManage(server), title: canManage(server) ? null : 'Instance servers are managed by instance administrators', onClick: () => edit(server) }, 'Edit'),
          h('button', { type: 'button', class: 'btn btn-sm btn-danger', disabled: !canManage(server), onClick: () => remove(server) }, 'Delete')));
    }));
  }

  const reload = loader(host, () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/mcp`), (list) => {
    servers = Array.isArray(list) ? list : [];
    return render();
  });
  scopeFilter.addEventListener('change', () => setChildren(host, render()));

  function baseBody(server) {
    return { name: server.name, transport: server.transport, command: server.command || null, args: server.args || [], url: server.url || null, enabled: server.enabled };
  }

  async function remove(server) {
    if (!(await confirmDialog({ title: `Remove MCP server “${server.name}”?`, message: 'New sessions no longer get this server; its stored secrets are deleted.', confirmLabel: 'Remove', danger: true }))) return;
    try {
      await ctx.api.del(`/api/mcp/${enc(server.id)}`);
      toast('Server removed.', { kind: 'ok' });
      reload();
    } catch (error) {
      errorToast(error, 'Could not remove server');
    }
  }

  /** Key/value editor for secrets; values are never prefilled. */
  function secretEditor(label, existingKeys) {
    const rows = [];
    const host2 = h('div', { class: 'kv-rows' });
    const replace = h('input', { type: 'checkbox', checked: existingKeys.length === 0 });
    const addRow = (key = '') => {
      const k = h('input', { class: 'input mono', value: key, placeholder: 'KEY', 'aria-label': `${label} name` });
      const v = h('input', { class: 'input mono', type: 'password', placeholder: key ? 'new value (required)' : 'value', autocomplete: 'off', 'aria-label': `${label} value` });
      const row = { k, v, el: null };
      row.el = h('div', { class: 'kv-row' }, k, v, iconButton('x', 'Remove', () => { rows.splice(rows.indexOf(row), 1); row.el.remove(); }));
      rows.push(row);
      host2.append(row.el);
    };
    const section = h('div', { class: 'stack-sm' });
    const renderSection = () => {
      setChildren(section,
        existingKeys.length ? h('div', { class: 'row-wrap small' }, h('span', { class: 'muted' }, 'Stored:'), existingKeys.map((k) => badge(`${k} ••••`))) : null,
        existingKeys.length ? h('label', { class: 'check small' }, replace, h('span', null, `Replace all ${label.toLowerCase()} (values are write-only; enter every key again)`)) : null,
        replace.checked ? [host2, h('div', null, h('button', { type: 'button', class: 'btn btn-sm', onClick: () => addRow() }, `Add ${label.toLowerCase().replace(/s$/, '')}`))] : null);
    };
    replace.addEventListener('change', () => {
      if (replace.checked && rows.length === 0) existingKeys.forEach((key) => addRow(key));
      renderSection();
    });
    renderSection();
    return {
      el: section,
      /** undefined = keep stored secrets; object = replace. Throws on invalid input. */
      value() {
        if (!replace.checked) return undefined;
        const out = {};
        for (const row of rows) {
          const key = row.k.value.trim();
          if (!key && !row.v.value) continue;
          if (!KEY_RE.test(key)) throw new Error(`Invalid ${label.toLowerCase()} name “${key}”.`);
          if (!row.v.value) throw new Error(`Enter a value for ${key}.`);
          out[key] = row.v.value;
        }
        return out;
      },
    };
  }

  function edit(server) {
    const name = h('input', { class: 'input', value: server ? server.name : '', maxlength: 64, autofocus: true, placeholder: 'quartio' });
    const transport = h('select', { class: 'select' }, h('option', { value: 'http' }, 'HTTP (Streamable)'), h('option', { value: 'stdio' }, 'stdio (command)'));
    transport.value = server ? server.transport : 'http';
    const url = h('input', { class: 'input mono', value: server ? server.url || '' : '', placeholder: 'https://mcp.example.com/mcp' });
    const command = h('input', { class: 'input mono', value: server ? server.command || '' : '', placeholder: 'npx' });
    const args = h('textarea', { class: 'textarea mono', rows: 3, placeholder: 'one argument per line', value: server && server.args ? server.args.join('\n') : '' });
    const scope = h('select', { class: 'select', disabled: Boolean(server) }, h('option', { value: 'sandbox' }, `This sandbox (${sandbox.name})`), instanceAdmin ? h('option', { value: 'instance' }, 'Whole instance') : null);
    scope.value = server ? server.scope : (route.query.get('scope') === 'instance' && instanceAdmin ? 'instance' : 'sandbox');
    const env = secretEditor('Environment variables', server ? server.env_keys || [] : []);
    const headers = secretEditor('Headers', server ? server.header_keys || [] : []);
    const httpFields = h('div', { class: 'stack' }, field('URL', url), h('div', { class: 'field' }, h('span', { class: 'field-label' }, 'Headers (secret)'), headers.el,
      h('span', { class: 'field-hint' }, 'Agent sessions receive HTTP secrets only as an Authorization: Bearer token; other headers are used by Test but not yet passed to the harnesses.')));
    const stdioFields = h('div', { class: 'stack' }, field('Command', command), field('Arguments', args), h('div', { class: 'field' }, h('span', { class: 'field-label' }, 'Environment (secret)'), env.el));
    const sync = () => { httpFields.hidden = transport.value !== 'http'; stdioFields.hidden = transport.value !== 'stdio'; };
    transport.addEventListener('change', sync);
    sync();
    openModal({
      title: server ? `Edit MCP server · ${server.name}` : 'Add MCP server',
      wide: true,
      body: h('div', { class: 'stack' },
        h('div', { class: 'form-grid' }, field('Name', name), field('Transport', transport), field('Scope', scope)),
        httpFields, stdioFields,
        h('p', { class: 'muted xs' }, 'Secrets are write-only: stored encrypted and never shown again. Leaving “Replace” unticked keeps the stored values. A sandbox server overrides an instance server of the same name.')),
      actions: [
        { label: 'Cancel', value: false },
        { label: server ? 'Save' : 'Add', kind: 'primary', run: async () => {
          const trimmed = name.value.trim();
          if (!NAME_RE.test(trimmed)) { toast('Use 1–64 letters, digits, “-” or “_” for the name.', { kind: 'warn' }); return false; }
          if (trimmed.toLowerCase() === 'yulius') { toast('“yulius” is reserved for the built-in Yulius tool server.', { kind: 'warn' }); return false; }
          const body = {
            name: name.value.trim(),
            transport: transport.value,
            command: transport.value === 'stdio' ? command.value.trim() || null : null,
            args: transport.value === 'stdio' ? args.value.split('\n').map((s) => s.trim()).filter(Boolean) : [],
            url: transport.value === 'http' ? url.value.trim() || null : null,
            enabled: server ? server.enabled : true,
          };
          if (!server) body.scope = scope.value;
          try {
            const envValue = transport.value === 'stdio' ? env.value() : undefined;
            if (envValue !== undefined) body.env = envValue;
            const headerValue = transport.value === 'http' ? headers.value() : undefined;
            if (headerValue !== undefined) body.headers = headerValue;
          } catch (error) {
            toast(error.message, { kind: 'warn' });
            return false;
          }
          if (server) await ctx.api.put(`/api/mcp/${enc(server.id)}`, body);
          else await ctx.api.post(`/api/sandboxes/${enc(sandbox.id)}/mcp`, body);
          toast(server ? 'Server saved.' : 'Server added.', { kind: 'ok' });
          reload();
          return true;
        } },
      ],
    });
  }

  return { el: frame.el, key: `mcp:${sandbox.id}:${route.query.get('scope') || ''}` };
}
