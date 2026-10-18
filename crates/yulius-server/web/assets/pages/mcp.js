// MCP page (MCP-001..003): sandbox servers plus inherited instance servers; test from the list.
// Adding and editing happen on the server's page (mcpedit.js, UI-038). Secrets are write-only:
// the UI shows key names only and never reads values back.

import { enc } from '../api.js';
import { h, icon, badge, iconBadge, itemCard, toast, confirmDialog, errorToast, toggle, setChildren, iconButton, emptyState, filterTabs } from '../dom.js';
import { navigate, replaceQuietly } from '../router.js';
import { allows } from '../status.js';
import { pageFrame, loader, createButton } from './common.js';
import { BRAND } from '../brand.js';

/** Logos of well-known servers (assets/logos, see its README), matched by name, URL or package. */
const LOGOS = [
  { src: '/assets/logos/quartio.svg', label: 'Quartio', test: /quartio/i, onDark: true }, // a light mark: always on a dark tile
  { src: '/assets/logos/apify.svg', label: 'Apify', test: /apify/i },
];

/** The server's logo when it is a known service, else an icon for its transport. */
function serverLogo(server) {
  const haystack = [server.name, server.url, server.command, ...(server.args || [])].filter(Boolean).join(' ');
  const known = LOGOS.find((logo) => logo.test.test(haystack));
  return known ? h('img', { class: known.onDark ? 'logo-on-dark' : null, src: known.src, alt: '', width: 22, height: 22 }) : icon(server.transport === 'http' ? 'globe' : 'terminal', { size: 18 });
}

/** Mount the MCP page; ?scope=instance pre-filters (admin "Instance MCP" entry). */
export function mountMcpPage(ctx, sandbox, route) {
  const write = allows(sandbox.access, 'write');
  const instanceAdmin = ctx.isInstanceAdmin();
  const base = `/s/${encodeURIComponent(sandbox.slug)}/mcp`;
  // Adding happens on its own page (UI-038); both Add buttons go there.
  const add = () => navigate(`${base}/new`);
  // The scope is kept in the address, so it comes back with the page (SPEC-113 KST-007).
  const scopeFilter = filterTabs({ label: 'Scope', value: route.query.get('scope') || '', onChange: (scope) => { replaceQuietly(scope ? `${base}?scope=${encodeURIComponent(scope)}` : base); setChildren(host, render()); },
    options: [{ value: '', label: 'All' }, { value: 'sandbox', label: 'This sandbox', icon: 'folder' }, { value: 'instance', label: 'Every sandbox', icon: 'globe' }] });
  const frame = pageFrame({
    title: 'MCP',
    sub: 'attached once, available to every Claude Code session on every subscription',
    actions: [createButton('Add server', add, { disabled: !write, title: write ? null : 'Requires write access' })],
  });
  const host = h('div');
  frame.body.append(h('div', { class: 'toolbar' }, scopeFilter.el, h('span', { class: 'muted small' }, `The ${BRAND.name} tool server is always attached and not listed here.`)), host);
  let servers = [];

  const canManage = (server) => write && (server.scope !== 'instance' || instanceAdmin);
  // Tests follow the write rule; stdio tests run outside the containment, so a contained
  // instance lets only instance administrators run them.
  const contained = Boolean(ctx.store.state.me && ctx.store.state.me.instance && ctx.store.state.me.instance.contained);
  // A local command from the repository's .mcp.json, which agents can write, is never tested on
  // the host until a person changes it (SPEC-218, #654).
  const imported = (server) => server.origin === 'mcp_json';
  const refused = (server) => imported(server) && server.transport === 'stdio';
  const canTest = (server) => canManage(server) && !refused(server) && (server.transport !== 'stdio' || !contained || instanceAdmin);

  function render() {
    const shown = servers.filter((s) => !scopeFilter.value || s.scope === scopeFilter.value);
    if (shown.length === 0) return servers.length
      ? emptyState({ iconName: 'plug', title: 'No server in this scope', text: 'Choose All to see every server.' })
      : emptyState({ iconName: 'plug', title: 'No MCP servers yet', text: 'Attach a tool server once and every Claude Code session gets it — for example the Quartio MCP (HTTP) or the Apify MCP (local command).', action: write ? createButton('Add server', add) : null });
    return h('div', { class: 'item-list' }, shown.map((server) => {
      const testResult = h('div', { class: 'small' });
      const manage = canManage(server);
      const testButton = h('button', { type: 'button', class: 'btn btn-sm', disabled: !canTest(server), title: canTest(server) ? 'Connect and list tools (HTTP ≤ 5 s, stdio ≤ 10 s)' : refused(server) ? 'From the repository’s .mcp.json: tested once a person has checked and changed its command' : server.scope === 'instance' && !instanceAdmin ? 'Servers for every sandbox are tested by instance administrators' : server.transport === 'stdio' && contained ? 'With isolation on, local-command tests need an instance administrator' : 'Requires write access', onClick: async () => {
        if (testButton.classList.contains('busy')) return; // one test at a time
        testButton.classList.add('busy');
        try {
          const result = await ctx.api.post(`/api/mcp/${enc(server.id)}/test`);
          setChildren(testResult, result.ok
            ? h('span', { class: 'row-wrap' }, badge('ok', 'ok'), h('span', { class: 'muted' }, `${(result.tools || []).length} tools: ${(result.tools || []).slice(0, 12).join(', ')}${(result.tools || []).length > 12 ? '…' : ''}`))
            : h('span', { class: 'row-wrap' }, badge('failed', 'danger'), h('span', { class: 'field-error' }, result.error || 'unknown error')));
        } catch (error) {
          errorToast(error, 'Test failed');
        } finally {
          testButton.classList.remove('busy');
        }
      } }, 'Test');
      const secrets = [...(server.env_keys || []).map((k) => `env ${k}`), ...(server.header_keys || []).map((k) => `header ${k}`)];
      return itemCard({
        iconEl: serverLogo(server),
        title: server.name,
        badges: [
          server.scope === 'instance' ? iconBadge('globe', 'Every sandbox', 'violet', 'Attached in every sandbox of this instance') : iconBadge('folder', 'This sandbox', '', 'Attached to this sandbox only'),
          iconBadge(server.transport === 'http' ? 'globe' : 'terminal', server.transport === 'http' ? 'HTTP' : 'Local command', '', server.transport === 'http' ? 'Remote server over HTTP' : 'Local command over stdio'),
          imported(server) ? iconBadge('download', 'From .mcp.json', 'warn', 'Added from the repository’s .mcp.json, which agents can write') : null,
        ].filter(Boolean),
        description: h('span', { class: 'mono' }, server.transport === 'http' ? server.url || '' : [server.command, ...(server.args || [])].filter(Boolean).join(' ')),
        extra: [secrets.length ? h('div', { class: 'chips' }, secrets.map((k) => iconBadge('lock', `${k} ••••`, '', 'Secret value is stored encrypted and never shown'))) : null, testResult],
        controls: [
          toggle({ checked: server.enabled, disabled: !manage, label: `${server.name} attached`, onChange: async (checked, input) => {
            try {
              await ctx.api.put(`/api/mcp/${enc(server.id)}`, { ...baseBody(server), enabled: checked });
              server.enabled = checked;
              input.closest('.item-card').classList.toggle('off', !checked);
            } catch (error) {
              input.checked = !checked;
              errorToast(error, 'Couldn’t update the server');
            }
          } }),
          testButton,
          iconButton('trash', `Delete ${server.name}`, () => remove(server), { class: 'btn-quiet-danger', disabled: !manage, title: manage ? `Delete ${server.name}` : 'Servers for every sandbox are managed by instance administrators' }),
        ],
        onOpen: () => navigate(`${base}/${encodeURIComponent(server.id)}`),
        openLabel: manage ? `Edit ${server.name}` : `Open ${server.name}`,
        dim: !server.enabled,
      });
    }));
  }

  const reload = loader(host, 'the MCP servers', () => ctx.api.get(`/api/sandboxes/${enc(sandbox.id)}/mcp`), (list) => {
    servers = Array.isArray(list) ? list : [];
    const counts = new Map();
    for (const s of servers) counts.set(s.scope, (counts.get(s.scope) || 0) + 1);
    scopeFilter.setCounts(counts, servers.length);
    return render();
  }, { shape: 'items' });

  function baseBody(server) {
    return { name: server.name, transport: server.transport, command: server.command || null, args: server.args || [], url: server.url || null, enabled: server.enabled };
  }

  async function remove(server) {
    if (!(await confirmDialog({ title: `Delete MCP server “${server.name}”?`, message: 'New sessions no longer get this server; its stored secrets are deleted.', confirmLabel: 'Delete', danger: true }))) return;
    try {
      await ctx.api.del(`/api/mcp/${enc(server.id)}`);
      toast(`MCP server ${server.name} deleted.`, { kind: 'ok' });
      reload();
    } catch (error) {
      errorToast(error, 'Couldn’t delete the server');
    }
  }

  return { el: frame.el, key: `mcp:${sandbox.id}:${route.query.get('scope') || ''}` };
}
