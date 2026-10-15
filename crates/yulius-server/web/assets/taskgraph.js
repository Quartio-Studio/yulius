// The Tasks page's Graph view (SPEC-164, ADR-0135): how a project's open tasks hang together.
//
// Everything comes from the task summaries the store already holds: `after` (a chain, or a start
// rule's anchor with `after.rule`), `held_by` (a supertask hold), `supertask`, `status`,
// `start_at` and `state_since`. `taskStructure` turns them into the queue in start order, the
// linked tasks laid out in columns (a task right of everything it waits for), the prepared tasks
// and the rest; `taskGraphView` draws it: cards are links over an SVG of arrows.

import { h, icon, statusMark, skeletonOf, bar } from './dom.js';
import { href } from './router.js';
import { activityLine, heldBy } from './status.js';

// A card's size and the gaps between columns and rows, in px.
const NODE_W = 232;
const NODE_H = 64;
const GAP_X = 56;
const GAP_Y = 14;
// Of two links between the same tasks, the one that says more is drawn.
const KIND_RANK = { hold: 0, after: 1, rule: 2 };
const SVG = 'http://www.w3.org/2000/svg';

const isOpen = (info) => info.group === 'needs' || info.group === 'working';
const ruleNumbers = (rule) => [...String(rule).matchAll(/#(\d+)/g)].map((m) => Number(m[1]));
const byNumber = (a, b) => a.number - b.number;

/**
 * The order the queue offers this project's queued tasks a session (`start_order` on the
 * server): supertasks first, then the longest queued first (`state_since` is when it was queued).
 */
export function queueOrder(items) {
  return items.filter(({ task }) => task.status === 'queued').sort((a, b) => Number(Boolean(b.task.supertask)) - Number(Boolean(a.task.supertask))
    || String(a.task.state_since || a.task.created_at).localeCompare(String(b.task.state_since || b.task.created_at)) || byNumber(a.task, b.task));
}

/**
 * Every link from the open tasks that still wait, one per pair of tasks: `{ from, to, kind }` by
 * task number. A started task keeps its `after`, but it waits for nothing any more.
 */
function linksOf(open) {
  const links = new Map();
  const add = (from, to, kind) => {
    if (!Number.isInteger(from) || from === to) return;
    const key = `${from}>${to}`;
    const seen = links.get(key);
    if (!seen || KIND_RANK[kind] > KIND_RANK[seen.kind]) links.set(key, { from, to, kind });
  };
  for (const { task } of open) {
    const after = task.status === 'chained' ? task.after : null;
    // A rule's anchor is one of the tasks it names: each named task gets the rule's arrow.
    if (after && after.rule) ruleNumbers(after.rule).forEach((n) => add(n, task.number, 'rule'));
    else if (after) add(after.number, task.number, 'after');
    heldBy(task).forEach((n) => add(n, task.number, 'hold'));
  }
  return [...links.values()];
}

/**
 * Columns by the longest path from the tasks nothing waits for, ignoring the links that close a
 * cycle (found by a depth-first walk in number order), so a cycle never hangs the layout.
 */
function columnsOf(numbers, links) {
  const out = new Map(numbers.map((n) => [n, []]));
  links.forEach((link) => out.get(link.from).push(link));
  const state = new Map(); // number → 1 on the walk's path, 2 finished
  const back = new Set();
  const walk = (n) => {
    state.set(n, 1);
    for (const link of out.get(n)) {
      if (state.get(link.to) === 1) back.add(link);
      else if (!state.has(link.to)) walk(link.to);
    }
    state.set(n, 2);
  };
  numbers.forEach((n) => { if (!state.has(n)) walk(n); });
  const forward = links.filter((link) => !back.has(link));
  const incoming = new Map(numbers.map((n) => [n, []]));
  forward.forEach((link) => incoming.get(link.to).push(link.from));
  const column = new Map();
  const depth = (n) => {
    if (!column.has(n)) column.set(n, incoming.get(n).reduce((most, from) => Math.max(most, depth(from) + 1), 0));
    return column.get(n);
  };
  numbers.forEach(depth);
  return { column, forward, back };
}

/**
 * Rows within each column: by number, then two sweeps placing each task near the mean row of what
 * it waits for (left to right) and of what waits for it (right to left).
 */
function rowsOf(numbers, column, forward) {
  const columns = [];
  numbers.forEach((n) => { (columns[column.get(n)] ||= []).push(n); });
  const row = new Map();
  const place = () => columns.forEach((list) => list.forEach((n, i) => row.set(n, i)));
  place();
  const mean = (list, fallback) => (list.length ? list.reduce((sum, n) => sum + row.get(n), 0) / list.length : fallback);
  const sweep = (list, neighbours) => {
    const key = new Map(list.map((n, i) => [n, mean(neighbours(n), i)]));
    list.sort((a, b) => key.get(a) - key.get(b) || a - b);
  };
  const preds = (n) => forward.filter((l) => l.to === n).map((l) => l.from);
  const succs = (n) => forward.filter((l) => l.from === n).map((l) => l.to);
  for (let c = 1; c < columns.length; c += 1) { sweep(columns[c], preds); place(); }
  for (let c = columns.length - 2; c >= 0; c -= 1) { sweep(columns[c], succs); place(); }
  return { row, height: Math.max(0, ...columns.map((list) => list.length)), width: columns.length };
}

/**
 * The structure of a project's tasks. `infoOf(task)` is status.js `taskInfo` with the store's
 * facts. Returns `queue` (queued items in start order), `graph` (`nodes` with their column, row
 * and kind — `open`, `context` for a linked task that is not open, `ghost` for one not in the
 * list —, `links`, and its size in cells), `prepared` and `alone` (the other open tasks).
 */
export function taskStructure(tasks, infoOf) {
  const items = tasks.map((task) => ({ task, info: infoOf(task) }));
  const known = new Map(items.map((item) => [item.task.number, item]));
  const open = items.filter(({ info }) => isOpen(info)).sort((a, b) => byNumber(a.task, b.task));
  const links = linksOf(open);
  const numbers = [...new Set(links.flatMap((l) => [l.from, l.to]))].sort((a, b) => a - b);
  const { column, forward, back } = columnsOf(numbers, links);
  const { row, height, width } = rowsOf(numbers, column, forward);
  const ghostTitle = (n) => open.map(({ task }) => task.after).find((after) => after && after.number === n && after.title)?.title || null;
  const nodes = numbers.map((n) => {
    const item = known.get(n);
    const kind = !item ? 'ghost' : isOpen(item.info) ? 'open' : 'context';
    return { number: n, item: item || null, title: item ? item.task.title : ghostTitle(n), kind, column: column.get(n), row: row.get(n) };
  });
  const inGraph = new Set(numbers);
  return {
    queue: queueOrder(open),
    graph: { nodes, links: links.map((link) => ({ ...link, back: back.has(link) })), width, height },
    prepared: open.filter(({ task }) => task.status === 'prepared' && !inGraph.has(task.number)),
    alone: open.filter(({ task }) => !inGraph.has(task.number) && task.status !== 'queued' && task.status !== 'prepared'),
  };
}

const taskPath = (sandbox, number) => `/s/${encodeURIComponent(sandbox.slug)}/tasks/${number}`;

/** A row of a list section: state mark, number, title, and its words; a link to the overview. */
function itemLink(sandbox, { task, info }, words, cls = 'tg-item') {
  const path = taskPath(sandbox, task.number);
  return h('a', { class: cls, href: href(path), dataset: { route: path } },
    h('span', { class: 'tg-mark' }, statusMark(info, 16)),
    h('span', { class: 'task-num' }, `#${task.number}`),
    h('span', { class: 'tg-title' }, task.title || `Task #${task.number}`),
    h('span', { class: 'tg-words' }, words));
}

function section(cls, iconName, title, count, sub, ...body) {
  return h('section', { class: ['tg-section', cls], 'aria-label': title },
    h('h2', { class: 'tg-head' }, icon(iconName, { size: 14 }), h('span', null, title), h('span', { class: 'tg-count' }, String(count))),
    sub ? h('p', { class: 'tg-sub' }, sub) : null,
    ...body);
}

function svgEl(name, attrs) {
  const el = document.createElementNS(SVG, name);
  for (const [key, value] of Object.entries(attrs)) el.setAttribute(key, String(value));
  return el;
}

/** The arrows: a curve from a card's right edge to the next one's left; a cycle's loops below. */
function edgesSvg(graph, at, size) {
  const svg = svgEl('svg', { class: 'tg-edges', width: size.width, height: size.height, 'aria-hidden': 'true' });
  const defs = svgEl('defs', {});
  const marker = svgEl('marker', { id: 'tg-arrow', viewBox: '0 0 8 8', refX: 7, refY: 4, markerWidth: 8, markerHeight: 8, orient: 'auto-start-reverse' });
  marker.append(svgEl('path', { d: 'M0 0L8 4L0 8z', class: 'tg-arrowhead' }));
  defs.append(marker);
  svg.append(defs);
  for (const link of graph.links) {
    const a = at.get(link.from);
    const b = at.get(link.to);
    const x1 = a.x + NODE_W;
    const y1 = a.y + NODE_H / 2;
    const x2 = b.x;
    const y2 = b.y + NODE_H / 2;
    const d = x2 > x1
      ? `M${x1} ${y1}C${x1 + GAP_X / 2} ${y1} ${x2 - GAP_X / 2} ${y2} ${x2} ${y2}`
      : `M${x1} ${y1}C${x1 + GAP_X} ${y1 + NODE_H} ${x2 - GAP_X} ${y2 + NODE_H} ${x2} ${y2}`;
    svg.append(svgEl('path', { d, class: `tg-edge tg-edge-${link.kind}`, 'marker-end': 'url(#tg-arrow)', 'data-from': link.from, 'data-to': link.to, 'data-kind': link.kind }));
  }
  return svg;
}

/** What a node says and its accessible name: the task, its state in words, what it waits for. */
function nodeView(sandbox, node, pos) {
  const style = { left: `${pos.x}px`, top: `${pos.y}px`, width: `${NODE_W}px`, height: `${NODE_H}px` };
  const dataset = { number: String(node.number) };
  if (!node.item) {
    return h('span', { class: ['tg-node', 'tg-ghost'], style, dataset, role: 'img', 'aria-label': `Task #${node.number}${node.title ? ` ${node.title}` : ''}, not in the list (archived or deleted)` },
      h('span', { class: 'tg-node-top' }, h('span', { class: 'task-num' }, `#${node.number}`), h('span', { class: 'tg-title' }, node.title || 'Not in the list')),
      h('span', { class: 'tg-words' }, 'Archived or deleted'));
  }
  const { task, info } = node.item;
  const words = activityLine(task, info);
  const path = taskPath(sandbox, task.number);
  return h('a', { class: ['tg-node', `tg-${node.kind}`, task.supertask && 'tg-supertask'], style, dataset: { ...dataset, route: path, tone: info.tone }, href: href(path),
    'aria-label': `Task #${task.number} ${task.title || ''}, ${words}`, title: `#${task.number} ${task.title || ''} — ${words}` },
  h('span', { class: 'tg-node-top' }, h('span', { class: 'tg-mark' }, statusMark(info, 14)), h('span', { class: 'task-num' }, `#${task.number}`), h('span', { class: 'tg-title' }, task.title || `Task #${task.number}`)),
  h('span', { class: 'tg-words' }, words));
}

function graphSection(sandbox, graph) {
  const at = new Map(graph.nodes.map((node) => [node.number, { x: node.column * (NODE_W + GAP_X), y: node.row * (NODE_H + GAP_Y) }]));
  // Room below the last row for a cycle's loop.
  const size = { width: graph.width * (NODE_W + GAP_X) - GAP_X, height: graph.height * (NODE_H + GAP_Y) - GAP_Y + (graph.links.some((l) => l.back) ? NODE_H : 0) };
  const canvas = h('div', { class: 'tg-canvas', style: { width: `${size.width}px`, height: `${size.height}px` } },
    // Cards column by column, top to bottom: the Tab order follows the picture.
    edgesSvg(graph, at, size), [...graph.nodes].sort((a, b) => a.column - b.column || a.row - b.row).map((node) => nodeView(sandbox, node, at.get(node.number))));
  const legend = h('p', { class: 'tg-legend' }, [['after', 'Runs after'], ['rule', 'Start rule'], ['hold', 'Supertask hold']].map(([kind, label]) => {
    const line = svgEl('svg', { class: 'tg-legend-line', width: 28, height: 8, 'aria-hidden': 'true' });
    line.append(svgEl('path', { d: 'M1 4H27', class: `tg-edge tg-edge-${kind}` }));
    return h('span', { class: 'tg-legend-item' }, line, label);
  }));
  const open = graph.nodes.filter((node) => node.kind === 'open').length;
  return section('tg-graph', 'workflow', 'Chains and rules', open, 'A task stands right of the tasks it waits for. Faded: done or ended.',
    h('div', { class: 'tg-frame', tabindex: '0', role: 'group', 'aria-label': 'Graph of the tasks that wait for each other' }, canvas), legend);
}

/** The Graph's shape while the tasks load (SPEC-120): two section outlines with rows of bars. */
export function taskGraphLoading(label) {
  return skeletonOf(label, [3, 2].map((rows) => h('div', { class: 'tg-section' },
    h('div', { class: 'tg-head' }, bar('lg')),
    h('div', { class: 'tg-list' }, Array.from({ length: rows }, (_, i) => h('div', { class: 'tg-item' }, bar([60, 45, 52][i])))))));
}

/**
 * The Graph view of a sandbox's tasks: Starts next, Chains and rules, Prepared, On their own;
 * a section with nothing is left out, and `empty` (an element) shows when all are.
 */
export function taskGraphView(sandbox, tasks, infoOf, empty) {
  const { queue, graph, prepared, alone } = taskStructure(tasks, infoOf);
  const parts = [];
  if (queue.length) {
    parts.push(section('tg-queue', 'st-queued', 'Starts next', queue.length, 'The order queued tasks get a free session. Sessions are shared with other projects.',
      h('ol', { class: 'tg-list' }, queue.map((item, i) => h('li', { class: 'tg-queue-item' },
        h('span', { class: 'tg-pos', 'aria-hidden': 'true' }, String(i + 1)),
        itemLink(sandbox, item, heldBy(item.task).length ? item.info.label : 'Waits for a free session'))))));
  }
  if (graph.nodes.length) parts.push(graphSection(sandbox, graph));
  if (prepared.length) {
    parts.push(section('tg-prepared', 'st-prepared', 'Prepared', prepared.length, 'Written now, started later.',
      h('div', { class: 'tg-list' }, prepared.map((item) => itemLink(sandbox, item, item.task.start_at ? item.info.label : 'When you decide')))));
  }
  if (alone.length) {
    parts.push(section('tg-alone', 'pulse', 'On their own', alone.length, 'Open tasks nothing waits for and that wait for nothing.',
      h('div', { class: 'tg-list' }, alone.map((item) => itemLink(sandbox, item, item.info.label)))));
  }
  return h('div', { class: 'tg' }, parts.length ? parts : empty);
}
