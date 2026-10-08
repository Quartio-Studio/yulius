// Design system page (/design): the living reference for the tokens and components, rendered by
// the same code the app uses. Change tokens.css or brand.js and this page shows the result in both
// themes; contrast ratios are computed live with the WCAG formula the repository check uses.

import { h, icon, iconButton, badge, iconBadge, toggle, meter, statusMark, itemCard, ICON_NAMES, setChildren, toast, keepFocus } from '../dom.js';
import { statusInfo } from '../status.js';
import { BRAND } from '../brand.js';
import { THEME_CHOICES, themeChoice, setThemeChoice, token, onThemeChange } from '../theme.js';
import { pageFrame } from './common.js';

const COLOR_GROUPS = [
  ['Surfaces', ['--term-bg', '--bg', '--surface', '--surface-2', '--surface-3', '--surface-hover', '--canvas']],
  ['Lines', ['--border', '--border-strong', '--border-control']],
  ['Ink', ['--text', '--text-2', '--muted', '--text-faint', '--text-disabled']],
  ['Accent', ['--accent', '--accent-hover', '--accent-strong', '--accent-fg', '--accent-fg-strong', '--accent-ink', '--accent-soft', '--accent-border', '--accent-track']],
  ['States', ['--ok', '--ok-fg', '--ok-soft', '--warn', '--warn-fg', '--warn-soft', '--danger', '--danger-fg', '--danger-soft', '--violet', '--violet-fg', '--agent']],
  ['Brand ramp', ['--brand-200', '--brand-300', '--brand-400', '--brand-500', '--brand-600', '--brand-700', '--brand-800']],
  ['Data', ['--series-1', '--series-1-hover', '--grid', '--axis']],
  ['Terminal', ['--term-fg', '--term-selection', '--ansi-red', '--ansi-green', '--ansi-yellow', '--ansi-blue', '--ansi-magenta', '--ansi-cyan', '--ansi-bright-black']],
];
const TYPE = [['--fs-2xl', '--lh-2xl', 'Display numbers'], ['--fs-xl', '--lh-xl', 'Page titles, wordmark'], ['--fs-lg', '--lh-lg', 'Section, panel and modal titles'], ['--fs-md', '--lh-md', 'Prose and item titles'], ['--fs', '--lh', 'Body — the UI default'], ['--fs-sm', '--lh-sm', 'Secondary lines, hints, meta'], ['--fs-xs', '--lh-xs', 'Captions, badges, counters']];
const SPACING = [2, 4, 6, 8, 12, 16, 20, 24, 32, 40, 48];
const RADII = ['--radius-sm', '--radius', '--radius-lg', '--radius-xl', '--radius-full'];
const CONTROLS = ['--control-sm', '--control-md', '--control-lg', '--control-xl'];
const SHADOWS = [['None', 'L0 canvas · L1 cards (border only)', 'none'], ['--shadow-sm', 'L2 small: labels, chips', 'var(--shadow-sm)'], ['--shadow', 'L2/L3: menus, popovers, toasts, modals', 'var(--shadow)'], ['--shadow-raised', 'Raised inputs: the composer', 'var(--shadow-raised)']];
const STATUSES = [
  { status: 'running', activity: 'needs_attention' }, { status: 'running', activity: 'waiting' }, { status: 'running', activity: 'working' },
  { status: 'queued' }, { status: 'waiting_capacity' }, { status: 'switching' }, { status: 'stopping' },
  { status: 'closed' }, { status: 'stopped_cancelled' }, { status: 'stopped_crashed' }, { status: 'failed_spawn_error' },
];

/** WCAG relative luminance contrast of two computed CSS colours ("rgb(…)" or "#…"). */
function contrast(fg, bg) {
  const parse = (value) => {
    const probe = document.createElement('span');
    probe.style.color = value;
    document.body.append(probe);
    const rgb = getComputedStyle(probe).color.match(/[\d.]+/g).map(Number);
    probe.remove();
    return rgb;
  };
  const [f, b] = [parse(fg), parse(bg)];
  const alpha = f[3] ?? 1;
  const mix = f.slice(0, 3).map((c, i) => c * alpha + b[i] * (1 - alpha));
  const lum = (rgb) => rgb.map((c) => { const v = c / 255; return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }).reduce((sum, v, i) => sum + v * [0.2126, 0.7152, 0.0722][i], 0);
  const [hi, lo] = [lum(mix), lum(b.slice(0, 3))].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

function section(title, description, ...content) {
  return h('section', { class: 'ds-section' }, h('div', { class: 'ds-head' }, h('h2', null, title), description ? h('p', { class: 'muted small' }, description) : null), content);
}

/** Mount the design system page. */
export function mountDesignPage(ctx) {
  const frame = pageFrame({ title: 'Design system', sub: 'tokens and components, live from tokens.css and brand.js' });
  const body = h('div', { class: 'stack ds' });
  frame.body.append(body);

  function render() {
    const themeSeg = h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Theme' }, THEME_CHOICES.map((choice) => h('label', null,
      h('input', { type: 'radio', name: 'ds-theme', value: choice.id, checked: themeChoice() === choice.id, onChange: () => setThemeChoice(choice.id) }), choice.name)));
    const swatch = (name) => {
      const value = token(name);
      const ink = ['--text', '--text-2', '--muted', '--text-faint', '--accent-fg', '--ok-fg', '--warn-fg', '--danger-fg', '--violet-fg', '--term-fg'].includes(name);
      const ratio = ink ? contrast(value, token(name === '--term-fg' ? '--term-bg' : '--surface')) : null;
      return h('div', { class: 'ds-swatch' },
        h('span', { class: 'ds-chip', style: { background: `var(${name})` } }),
        h('span', { class: 'ds-swatch-text' }, h('code', null, name), h('span', { class: 'muted xs mono' }, value),
          ratio ? h('span', { class: ['xs', ratio >= 4.5 ? 'ds-pass' : 'ds-fail'] }, `${ratio.toFixed(2)}:1 on ${name === '--term-fg' ? 'terminal' : 'surface'}`) : null));
    };
    // The theme switch rebuilds the page; keyboard focus stays on the same control.
    keepFocus(body, () => setChildren(body,
      section('Brand', 'brand.js: name, tagline and mark; colours are the --brand-* ramp in tokens.css.',
        h('div', { class: 'row-wrap ds-brand' },
          h('span', { class: 'sidebar-brand', style: { padding: 0 } }, h('span', { class: 'brand-logo' }, icon('logo', { size: 28 })), h('span', { class: 'brand-text' }, h('span', { class: 'brand-name' }, BRAND.name), h('span', { class: 'brand-by' }, BRAND.tagline))),
          h('span', { class: 'muted small' }, 'Theme'), themeSeg)),
      section('Colour roles', 'Components use roles only. Ink shows its contrast on the surface (4.5:1 needed).',
        COLOR_GROUPS.map(([name, list]) => h('div', { class: 'ds-group' }, h('h3', null, name), h('div', { class: 'ds-swatches' }, list.map(swatch))))),
      section('Type', 'One scale; each size has its line height. Weights 400, 500, 600.',
        TYPE.map(([fs, lh, use]) => h('div', { class: 'ds-type' },
          h('span', { style: { fontSize: `var(${fs})`, lineHeight: `var(${lh})`, fontWeight: 500 } }, 'Build something great'),
          h('span', { class: 'muted xs mono' }, `${fs} ${token(fs)} / ${token(lh)} — ${use}`)))),
      section('Spacing, radii, controls', 'Spacing on a 4 px scale; radii by role; four control heights.',
        h('div', { class: 'ds-spacing' }, SPACING.map((n) => h('div', { class: 'ds-space' }, h('span', { class: 'ds-space-bar', style: { width: `${n}px` } }), h('span', { class: 'xs mono muted' }, `${n}`)))),
        h('div', { class: 'row-wrap' }, RADII.map((r) => h('div', { class: 'ds-radius', style: { borderRadius: `var(${r})` } }, h('code', null, r)))),
        h('div', { class: 'row-wrap ds-controls' }, CONTROLS.map((c) => h('button', { type: 'button', class: 'btn', style: { height: `var(${c})` } }, `${c} ${token(c)}`)))),
      section('Elevation and motion', 'Three levels; three durations on one easing curve, all off under reduced motion.',
        h('div', { class: 'row-wrap' }, SHADOWS.map(([name, use, value]) => h('div', { class: 'ds-elev', style: { boxShadow: value } }, h('code', null, name), h('span', { class: 'muted xs' }, use)))),
        h('div', { class: 'muted small mono' }, `--duration-fast ${token('--duration-fast')} · --duration ${token('--duration')} · --duration-slow ${token('--duration-slow')} · --ease ${token('--ease')}`)),
      section('Icons', `${ICON_NAMES.length} icons on a 24-unit grid, one stroke (--icon-stroke ${token('--icon-stroke')}) at 12, 16 and 20 px.`,
        h('div', { class: 'ds-icons' }, ICON_NAMES.map((name) => h('div', { class: 'ds-icon', title: name }, icon(name, { size: 20 }), h('span', { class: 'xs muted' }, name))))),
      section('Task states', 'statusInfo(): one shape and a word per state; colour is never the only cue.',
        h('div', { class: 'ds-states' }, STATUSES.map((task) => { const info = statusInfo(task); return h('div', { class: 'row nowrap' }, statusMark(info, 16), h('span', null, info.label), h('span', { class: 'muted xs' }, info.group)); }))),
      section('Buttons and inputs', null,
        h('div', { class: 'row-wrap' },
          h('button', { type: 'button', class: 'btn btn-primary' }, 'Primary'),
          h('button', { type: 'button', class: 'btn' }, 'Secondary'),
          h('button', { type: 'button', class: 'btn btn-ghost' }, 'Ghost'),
          h('button', { type: 'button', class: 'btn btn-danger' }, 'Danger'),
          h('button', { type: 'button', class: 'btn', disabled: true }, 'Disabled'),
          h('button', { type: 'button', class: 'btn btn-sm' }, 'Small'),
          iconButton('gear', 'Icon button', () => toast('Icon button')),
          iconButton('trash', 'Quiet danger', () => toast('Quiet danger'), { class: 'btn-quiet-danger' })),
        h('div', { class: 'row-wrap' },
          h('input', { class: 'input ds-input', placeholder: 'Input with placeholder' }),
          h('select', { class: 'select ds-select', 'aria-label': 'Sample select' }, h('option', null, 'Select')),
          h('div', { class: 'seg' }, ['One', 'Two', 'Three'].map((label, i) => h('label', null, h('input', { type: 'radio', name: 'ds-seg', checked: i === 0 }), label))),
          toggle({ checked: true, label: 'Sample switch, on', onChange: () => {} }),
          toggle({ checked: false, label: 'Sample switch, off', onChange: () => {} }),
          h('button', { type: 'button', class: 'chip-btn' }, icon('target', { size: 14 }), h('span', { class: 'chip-label' }, 'Picker chip'), icon('down', { size: 12 })),
          h('span', { class: 'kbd' }, '⌘⇧O'))),
      section('Badges, banners, meters', null,
        h('div', { class: 'row-wrap' }, badge('neutral'), badge('accent', 'accent'), badge('ok', 'ok'), badge('warn', 'warn'), badge('danger', 'danger'), badge('violet', 'violet'),
          iconBadge('globe', 'Whole instance', 'violet'), iconBadge('lock', 'Secret', ''), h('span', { class: 'plan-badge' }, 'Max 20x')),
        h('div', { class: 'stack-sm' },
          h('div', { class: 'banner banner-info' }, icon('globe'), h('span', { class: 'banner-text' }, 'Info banner')),
          h('div', { class: 'banner banner-warn' }, icon('warn'), h('span', { class: 'banner-text' }, 'Warning banner')),
          h('div', { class: 'banner banner-danger' }, icon('warn'), h('span', { class: 'banner-text' }, 'Danger banner'))),
        h('div', { class: 'status-grid' }, meter(32, { label: 'Normal', detail: 'below 70 %' }), meter(78, { label: 'Warning', detail: '70–90 %' }), meter(96, { label: 'Danger', detail: 'above 90 %' }), meter(null, { label: 'Unknown', detail: 'never shown as zero' }))),
      section('Item card', 'itemCard(): the whole card opens the item; controls keep their own clicks.',
        h('div', { class: 'item-list' },
          itemCard({ iconName: 'workflow', title: 'Feature', badges: [iconBadge('globe', 'Whole instance', 'violet'), iconBadge('check', 'Tests required', '')], description: 'Spec, implementation, tests and a pull request.', extra: h('ol', { class: 'steps-line' }, ['Write spec', 'Implement', 'Test'].map((s, i) => h('li', null, h('span', { class: 'step-num' }, String(i + 1)), s))), controls: [iconButton('trash', 'Delete', () => toast('Delete'), { class: 'btn-quiet-danger' })], onOpen: () => toast('Opened') }),
          itemCard({ iconName: 'book', title: 'A disabled item', description: 'Dimmed, still readable.', dim: true, controls: [toggle({ checked: false, label: 'A disabled item enabled', onChange: () => {} })], onOpen: () => toast('Opened') }))),
    ));
  }

  render();
  const off = onThemeChange(() => render());
  return { el: frame.el, destroy: off };
}
