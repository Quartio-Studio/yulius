// Brand moments (SPEC-001 UI-041): a slow shader where a brand shows its mark — in Roma, smoke
// rising in the composer's arched niche, like incense in a temple niche. The vendored `shaders`
// library (WebGPU; assets/vendor/shaders, its telemetry removed) is fetched only when the niche is
// on screen, the browser has WebGPU, and the person has asked for neither reduced motion nor data
// saving. It pauses while the tab is hidden and follows the theme. Without it the niche stays the
// static arch it already is.

import { hexToken, onThemeChange } from './theme.js';

const SMOKE_ID = 'smoke';
const reducedMotion = () => window.matchMedia('(prefers-reduced-motion: reduce)').matches;
const saveData = () => Boolean(navigator.connection && navigator.connection.saveData);

/** Smoke in `niche` while it is shown; returns a function that stops it. */
export function brandSmoke(niche) {
  if (!niche || !('gpu' in navigator)) return () => {};
  let canvas = null;
  let shader = null;
  let starting = false;
  let stopped = false;
  let failed = false;
  // Fresh smoke in the secondary ink, ageing into the niche's own surface: wisps, not a column.
  const colors = () => ({ colorA: hexToken('--text-2', niche), colorB: hexToken('--surface', niche) });
  const shown = () => niche.isConnected && getComputedStyle(niche).display !== 'none';

  function stop() {
    if (shader) shader.destroy();
    shader = null;
    if (canvas) canvas.remove();
    canvas = null;
    niche.classList.remove('has-fx');
  }

  async function start() {
    if (shader || starting || failed || stopped || reducedMotion() || saveData() || !shown()) return;
    starting = true;
    canvas = document.createElement('canvas');
    canvas.className = 'brand-niche-fx';
    niche.prepend(canvas);
    try {
      // No GPU adapter (blocked, software-only, unsupported): skip the 5 MB library altogether.
      if (!(await navigator.gpu.requestAdapter({ powerPreference: 'high-performance' }))) { failed = true; stop(); return; }
      const { createShader } = await import('./vendor/shaders/js/createShader.js');
      if (stopped || !shown()) { stop(); return; }
      shader = await createShader(canvas, {
        components: [{ type: 'Smoke', id: SMOKE_ID, props: {
          ...colors(),
          emitFrom: { x: 0.5, y: 1.04 }, direction: 0, speed: 5, spread: 42, emitRadius: 0.06,
          intensity: 0.3, dissipation: 0.42, detail: 38, gravity: -0.55, colorDecay: 0.9, mouseInfluence: 0.04, mouseRadius: 0.25,
        } }],
      }, { disableTelemetry: true, onError: () => { failed = true; stop(); } });
      if (stopped || failed) { stop(); return; }
      if (document.hidden) shader.pause();
      niche.classList.add('has-fx');
    } catch {
      failed = true;
      stop();
    } finally {
      starting = false;
    }
  }

  // The niche shows only with a brand that uses it (data-brand, set by the brand lab), and the
  // theme changes the smoke's colours: watch both, start or stop as they change.
  const observer = new MutationObserver(() => {
    if (!shown()) stop();
    else if (shader) shader.update(SMOKE_ID, colors());
    else start();
  });
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-brand', 'data-theme'] });
  const offTheme = onThemeChange(() => { if (shader) shader.update(SMOKE_ID, colors()); });
  const onVisibility = () => { if (shader) { if (document.hidden) shader.pause(); else shader.resume(); } };
  document.addEventListener('visibilitychange', onVisibility);
  const motion = window.matchMedia('(prefers-reduced-motion: reduce)');
  const onMotion = () => { if (motion.matches) stop(); else start(); };
  motion.addEventListener('change', onMotion);
  start();

  return () => {
    stopped = true;
    observer.disconnect();
    offTheme();
    document.removeEventListener('visibilitychange', onVisibility);
    motion.removeEventListener('change', onMotion);
    stop();
  };
}
