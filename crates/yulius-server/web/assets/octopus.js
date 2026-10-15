// The octopus of a watched task (SPEC-179 WAT-006): an electro-blue pixel octopus that lives in a
// lane stuck to the top of the live transcript, crawling from one top corner to the other and
// resting there. Its face is a reference to a film's little octopus: two dot eyes set wide and an ε
// mouth, nothing else. Its eight arms are drawn per frame (crawling, resting, waving) as pixel art;
// the stylesheet steps through the frames (`steps()`, stopped under reduced motion) and this script
// moves it between the corners with the Web Animations API, as the Sessions view's cockroach.

import { h } from './dom.js';

const SVG_NS = 'http://www.w3.org/2000/svg';
const svgEl = (name, attributes, ...children) => {
  const node = document.createElementNS(SVG_NS, name);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, String(value));
  node.append(...children);
  return node;
};

/** The sprite's grid: the head on top, the arms fanning out below it. */
export const OCTOPUS = { width: 48, height: 36 };
const CX = 24; // the head's centre line
const HEAD = { cy: 10.6, rx: 10.4, ry: 9.2 }; // its outline's top row is the grid's first
// Each arm: where it leaves the head (degrees round the rim from straight down; − is left), how it
// fans out (degrees from straight down), how long it is and whether it is behind (shaded). Drawn in
// this order, back to front: the outer arms behind, the middle ones in front. The outer arms' tips
// curl back in and up; the middle ones' curl outward, so no two tips meet.
const ARMS = [
  { rim: -58, fan: -82, length: 14, back: true }, { rim: 58, fan: 82, length: 14, back: true },
  { rim: -40, fan: -55, length: 15, back: true }, { rim: 40, fan: 55, length: 15, back: true },
  { rim: -24, fan: -30, length: 15, back: false, out: true }, { rim: 24, fan: 30, length: 15, back: false, out: true },
  { rim: -8, fan: -12, length: 13, back: false, out: true }, { rim: 8, fan: 12, length: 13, back: false, out: true },
];

/** Pixels of a disc of radius `r` centred at (x, y). */
function disc(into, x, y, r) {
  for (let yy = Math.floor(y - r) - 1; yy <= Math.ceil(y + r) + 1; yy += 1) {
    for (let xx = Math.floor(x - r) - 1; xx <= Math.ceil(x + r) + 1; xx += 1) {
      if ((xx + 0.5 - x) ** 2 + (yy + 0.5 - y) ** 2 <= r * r) into.add(`${xx},${yy}`);
    }
  }
}

/** The pixels around a set (its outline), four-connected. */
function ring(cells) {
  const out = new Set();
  for (const key of cells) {
    const [x, y] = key.split(',').map(Number);
    for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) {
      const next = `${x + dx},${y + dy}`;
      if (!cells.has(next)) out.add(next);
    }
  }
  return out;
}

const smooth = (t, from, to) => Math.max(0, Math.min(1, (t - from) / (to - from)));

/**
 * One arm's pixels: a centreline walked in quarter pixels from its root under the head, tapering
 * from 3 pixels wide to 1. It bends, and its tip curls as an octopus's does (`out`: outward).
 * `phase` and `amp` send a wave from the root to the tip (the arm pushing as it crawls); `raise`
 * points it up and out instead, to wave.
 */
function arm(index, { phase, amp, curl, raise = 0 }) {
  const { rim, fan, length, out = false } = ARMS[index];
  const side = Math.sign(fan);
  const at = (rim * Math.PI) / 180;
  let x = CX + Math.sin(at) * HEAD.rx * 0.72;
  let y = HEAD.cy + Math.cos(at) * HEAD.ry * 0.62;
  const base = raise ? side * raise : (fan * Math.PI) / 180;
  const steps = length * 4;
  const cells = new Set();
  const centre = [];
  for (let k = 0; k <= steps; k += 1) {
    const t = k / steps;
    disc(cells, x, y, 1.65 * (1 - t) ** 1.2 + 0.5 * t);
    centre.push([x, y, t]);
    const wiggle = amp * Math.sin(phase - t * 5 + index * 0.9) * t;
    const tip = curl * smooth(t, 0.5, 1) ** 1.6 * 2.6;
    const direction = base + side * (out ? -0.25 : 0.35) * t + (out ? side * tip * 0.7 : -side * tip) + wiggle;
    x += Math.sin(direction) * 0.25;
    y += Math.cos(direction) * 0.25;
  }
  return { cells, centre, side };
}

/** A frame's arms as a pixel map (key → part), drawn back to front with an outline each. */
function armsMap(poses) {
  const map = new Map();
  ARMS.forEach(({ back }, index) => {
    const { cells, centre, side } = arm(index, poses[index]);
    for (const key of ring(cells)) map.set(key, 'ink');
    for (const key of cells) map.set(key, back ? 'shade' : 'body');
    // Suckers on the inside of the curling tip, a pixel apart.
    let last = -1;
    for (const [cx, cy, t] of centre) {
      if (t < 0.5 || t > 0.9 || t - last < 0.11) continue;
      const key = `${Math.floor(cx + side * 0.7)},${Math.floor(cy + 0.4)}`;
      if (cells.has(key)) { map.set(key, 'sucker'); last = t; }
    }
  });
  return map;
}

/** The head (the mantle), its outline, shade, highlight and the face: dot eyes and an ε mouth. */
function headMap() {
  const cells = new Set();
  for (let y = 0; y < OCTOPUS.height; y += 1) {
    for (let x = 0; x < OCTOPUS.width; x += 1) {
      if (((x + 0.5 - CX) / HEAD.rx) ** 2 + ((y + 0.5 - HEAD.cy) / HEAD.ry) ** 2 <= 1) cells.add(`${x},${y}`);
    }
  }
  const map = new Map();
  for (const key of ring(cells)) map.set(key, 'ink');
  for (const key of cells) {
    const [x, y] = key.split(',').map(Number);
    const lit = ((x + 0.5 - CX + 2.6) / HEAD.rx) ** 2 + ((y + 0.5 - HEAD.cy + 2) / HEAD.ry) ** 2;
    map.set(key, lit > 0.8 ? 'shade' : 'body');
  }
  for (const key of ['18,4', '19,4', '17,5', '18,5', '17,6']) map.set(key, 'glow');
  return map;
}

/** The face, on its own grid of half pixels: one dot per eye, the mouth an ε of five rows. */
export const FACE = {
  eyes: [[41, 17], [53, 17]],
  // Columns and rows in half pixels: the ε's top and bottom strokes reach right, its two bumps left.
  mouth: ['.##', '#..', '.#.', '#..', '.##'],
  mouthAt: [46, 20],
};

function rects(map, part, offset = 0) {
  const out = [];
  for (let y = 0; y < OCTOPUS.height + 2; y += 1) {
    for (let x = -2; x < OCTOPUS.width + 2; x += 1) {
      if (map.get(`${x},${y}`) !== part) continue;
      let run = 1;
      while (map.get(`${x + run},${y}`) === part) run += 1;
      out.push(svgEl('rect', { class: `octo-${part}`, x: x + offset, y, width: run, height: 1 }));
      x += run - 1;
    }
  }
  return out;
}
const PARTS = ['ink', 'shade', 'body', 'sucker', 'glow'];
const layer = (map, attributes) => svgEl('g', attributes, ...PARTS.flatMap((part) => rects(map, part)));

/** Poses of the eight arms for a kind of frame. */
function poses(kind, frame) {
  return ARMS.map((_, index) => {
    if (kind === 'crawl') return { phase: (frame * Math.PI) / 2, amp: 0.42, curl: 0.8 };
    if (kind === 'wave' && index === 1) return { phase: (frame * Math.PI) / 2, amp: 0.25, curl: 0.55, raise: frame ? 2.55 : 2.2 };
    return { phase: (frame * Math.PI) / 2, amp: 0.16, curl: 1 };
  });
}

/** The octopus as SVG: its arm frames (crawl ×4, rest ×4, wave ×2), its head and its face. */
export function octopusSprite() {
  const frames = [];
  for (const [kind, count] of [['crawl', 4], ['rest', 4], ['wave', 2]]) {
    for (let frame = 0; frame < count; frame += 1) {
      frames.push(layer(armsMap(poses(kind, frame)), { class: `octo-frame octo-${kind} octo-${kind}-${frame}` }));
    }
  }
  const face = [];
  for (const [x, y] of FACE.eyes) face.push(svgEl('rect', { class: 'octo-eye', x: x / 2, y: y / 2, width: 1, height: 1 }));
  const mouth = [];
  FACE.mouth.forEach((row, dy) => [...row].forEach((c, dx) => {
    if (c === '#') mouth.push(svgEl('rect', { class: 'octo-mouth', x: (FACE.mouthAt[0] + dx) / 2, y: (FACE.mouthAt[1] + dy) / 2, width: 0.5, height: 0.5 }));
  }));
  return svgEl('svg', { class: 'octo-sprite', viewBox: `0 0 ${OCTOPUS.width} ${OCTOPUS.height}`, 'aria-hidden': 'true', focusable: 'false', 'shape-rendering': 'crispEdges' },
    svgEl('g', { class: 'octo-arms' }, ...frames),
    svgEl('g', { class: 'octo-head' }, layer(headMap(), { class: 'octo-mantle' }),
      svgEl('g', { class: 'octo-look' }, svgEl('g', { class: 'octo-eyes' }, ...face), svgEl('g', { class: 'octo-mouth-group' }, ...mouth))));
}

const reducedMotion = () => Boolean(window.matchMedia?.('(prefers-reduced-motion: reduce)').matches);

/** How long it rests in a corner (ms): the first time briefly, then 3–7 s. */
export function restDelay(first, random = Math.random) {
  const [min, max] = first ? [1200, 2400] : [3000, 7000];
  return min + Math.floor(random() * (max - min));
}
/** Its speed along the top, in pixels a second. */
export const CRAWL_SPEED = 90;

/**
 * The octopus in `lane` (a positioned box across the top of the view), resting in the top-right
 * corner, then crawling to the other corner and back for as long as it lives. Its element says what
 * it does: `is-crawling` or `is-resting` (and `is-waving` for a while), `is-left` or `is-right` for
 * where it faces, `data-corner` for the corner it rests in. Under reduced motion it rests still in
 * the top-right corner (`is-still`). `stop()` ends it and removes it. `timer` stands in for timeouts in tests.
 */
export function createOctopus(lane, { random = Math.random, timer = { set: (fn, ms) => setTimeout(fn, ms), clear: (id) => clearTimeout(id) } } = {}) {
  const el = h('div', { class: 'octo is-resting is-left', 'aria-hidden': 'true', dataset: { corner: 'right' } }, octopusSprite());
  lane.append(el);
  let corner = 'right';
  let handle = null;
  let run = null;
  let stopped = false;
  const spot = (which) => (which === 'right' ? Math.max(0, lane.clientWidth - el.offsetWidth) : 0);
  const place = () => { el.style.transform = `translateX(${spot(corner)}px)`; };
  const resize = typeof ResizeObserver === 'function' ? new ResizeObserver(() => { if (!run) place(); }) : null;
  if (resize) resize.observe(lane);
  place();

  function set(state) {
    el.classList.toggle('is-crawling', state === 'crawl');
    el.classList.toggle('is-resting', state !== 'crawl');
  }

  function rest(first) {
    set('rest');
    el.dataset.corner = corner;
    // It faces into the view from its corner, and waves now and then.
    el.classList.toggle('is-left', corner === 'right');
    el.classList.toggle('is-right', corner === 'left');
    if (random() < 0.6) {
      handle = timer.set(() => {
        if (stopped) return;
        el.classList.add('is-waving');
        handle = timer.set(() => { el.classList.remove('is-waving'); handle = timer.set(crawl, 400 + Math.floor(random() * 1200)); }, 1600);
      }, 500);
      return;
    }
    handle = timer.set(crawl, restDelay(first, random));
  }

  function crawl() {
    if (stopped || !el.isConnected) return;
    const to = corner === 'right' ? 'left' : 'right';
    const [from, end] = [spot(corner), spot(to)];
    el.classList.remove('is-waving');
    el.classList.toggle('is-left', to === 'left');
    el.classList.toggle('is-right', to === 'right');
    delete el.dataset.corner;
    set('crawl');
    run = el.animate([{ transform: `translateX(${from}px)` }, { transform: `translateX(${end}px)` }], { duration: Math.max(600, (Math.abs(end - from) / CRAWL_SPEED) * 1000), easing: 'linear' });
    run.finished.then(() => {
      if (stopped) return;
      run = null;
      corner = to;
      place();
      rest(false);
    }, () => {});
  }

  if (reducedMotion()) el.classList.add('is-still');
  else rest(true);
  return {
    el,
    stop() {
      stopped = true;
      timer.clear(handle);
      if (run) run.cancel();
      if (resize) resize.disconnect();
      el.remove();
    },
  };
}
