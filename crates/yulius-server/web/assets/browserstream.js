// The sandbox's Chrome, streamed (BRW-004..008; SPEC-002 ADR-15). The browser runs headful on a
// virtual display on the server; this view shows the display as H.264 video decoded with
// WebCodecs onto a canvas and sends the user's pointer and keys, which the server injects as
// real input (XTest) — so cross-origin frames, canvases and captchas work as in a local browser.
// Agents drive the same Chrome; their cursor is drawn over the canvas (BRW-007).
//
// - Video: `video_config` configures a VideoDecoder (`optimizeForLatency`), binary frames
//   (u8 4, flags, u64 µs, access unit) are acknowledged as they arrive (`ack`: the server keeps
//   only about a round trip of frames on the way and adapts the bitrate, F 2026-10-10), all
//   decoded, and the newest decoded picture is drawn (at once when the decoder is idle, else on
//   the next animation frame; older ones are dropped). Frames arrive in bursts after a keyframe
//   or a network hiccup: decoding them all is cheap, skipping to a keyframe froze the view for
//   seconds. A decoder that fails or falls far behind restarts and asks for a keyframe
//   (`keyframe`). Without WebCodecs or H.264 (or a server without a display) the view asks for
//   JPEG frames (u8 3 + tab id + JPEG): the labelled fallback.
// - Cursor: the user's own pointer is the local one (no round trip); the video has none.
// - Size: the panel reports its size (`viewport`); the server shares one viewport between
//   viewers, so a panel sized by someone else shows the page scaled, with a notice.
// - Keys: a hidden textarea holds the focus so IME, dead keys, emoji pickers and phone keyboards
//   work: printable keys go as keys, composed text as `char` text. On macOS ⌘ maps to Ctrl for
//   editing shortcuts; paste sends the user's clipboard as text and copy brings the page's
//   selection back to the user's clipboard.
// - Touch: a tap clicks, a drag scrolls.
//
// The panel (browser.js) owns the tab strip and the address bar; this view owns the socket, the
// tab list and the stage.

import { enc } from './api.js';
import { h, toast, focusNextRegion, skeleton } from './dom.js';
import { ReconnectingSocket, parseBrowserFrame, CLOSE_REVOKED, CLOSE_SERVER_ERROR } from './termsocket.js';

const FRAME_VIDEO = 4;
const MOUSE_BUTTONS = ['left', 'middle', 'right', 'back', 'forward'];
const AGENT_CURSOR_IDLE_MS = 6000;
/** Decoder queue length (about 1.5 s at 60 fps) beyond which the decoder restarts at a fresh
 *  keyframe: this device cannot keep up. Short bursts are normal and decoded. */
const MAX_DECODE_BACKLOG = 90;
/** Shortest time between two keyframe requests. */
const KEYFRAME_REQUEST_MS = 1000;
/** A touch that moves farther than this (CSS px) scrolls instead of clicking. */
const TAP_SLOP = 8;
/** Editing shortcuts that ⌘ sends as Ctrl on macOS. */
const MAC_EDIT_KEYS = new Set(['a', 'c', 'x', 'z', 'y', 'f', 'g', 'b', 'i', 'u', 'k', 's', 'p', 'enter']);
/** The user's own browser shortcuts, never sent to the page. */
const OWN_SHORTCUTS = new Set(['r', 'w', 't', 'q', 'l', 'n', 'tab', '[', ']']);
const IS_MAC = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || '');

/** Decode a video frame: u8 4, u8 flags (bit 0 keyframe), u64 big-endian µs, access unit. */
export function parseVideoFrame(buffer) {
  if (!(buffer instanceof ArrayBuffer) || buffer.byteLength < 10) return null;
  const view = new DataView(buffer);
  if (view.getUint8(0) !== FRAME_VIDEO) return null;
  return { keyframe: (view.getUint8(1) & 1) === 1, timestamp: Number(view.getBigUint64(2, false)), data: new Uint8Array(buffer, 10) };
}

function base64Bytes(text) {
  try { return Uint8Array.from(atob(text || ''), (c) => c.charCodeAt(0)); } catch { return null; }
}

/** Whether this browser has WebCodecs; whether it decodes the announced codec shows when the
 *  decoder is configured (a `NotSupportedError` turns the view to JPEG frames). */
export function hasWebCodecs() {
  return typeof VideoDecoder === 'function' && typeof EncodedVideoChunk === 'function';
}

/**
 * The streamed sandbox Chrome.
 * hooks: onTabs(tabs), onAgent(message), onActionRequested(message), onCreated(tabId), onConnected().
 */
export function createStreamView(ctx, sandbox, hooks) {
  let tabs = [];
  let watched = null; // the tab the panel shows
  let socket = null;
  let expectNew = 0;
  let knownIds = new Set();
  let lastError = null;
  let mode = 'video'; // what this view asks for
  let streamMode = null; // what the server sends: 'video' | 'pixels'
  let streamReason = null;
  let decoder = null;
  let decoderConfig = null;
  let waitingKey = true;
  let framesReceived = 0; // video frames on this connection (acknowledged one by one)
  let keyframeAsked = 0; // when a keyframe was last asked for
  let pendingPicture = null; // the newest decoded picture not drawn yet
  let pictureFrame = 0; // animation frame that draws it
  let jpegDecoding = false;
  let jpegQueued = null;
  let hasPicture = false;
  let serverViewport = null; // {width, height}: the shared viewport in CSS px
  let sentViewport = null;
  let viewportTimer = 0;
  // A viewport was sent and the server has not answered yet: a size that differs is ours in
  // flight, not another viewer's.
  let awaitingViewport = false;
  let awaitingTimer = 0;
  let moveFrame = 0;
  let pendingMove = null;
  let lastMove = null; // the position last sent
  let wheelFrame = 0;
  let wheel = { dx: 0, dy: 0, x: 0, y: 0 };
  let touch = null; // {id, x, y, lastX, lastY, scrolling}
  let pressed = 0; // buttons held
  let lastAgent = null;
  let agentTimer = 0;
  let copiedTimer = 0;
  let pendingCopy = null; // {resolve, reject, timer}
  let compositionText = null;
  let fit = 1;
  const held = new Map(); // code → key of keys sent down and not yet up

  const canvas = h('canvas', { class: 'bp-canvas', width: 1280, height: 800, 'aria-hidden': 'true' });
  const g = canvas.getContext('2d', { alpha: false, desynchronized: true }) || canvas.getContext('2d');
  // The focus target for keys: a textarea, so composition and phone keyboards work.
  const keys = h('textarea', { class: 'bp-keys', 'aria-label': 'Sandbox Chrome page: keys go to the page', 'aria-description': 'Press F6 to move to the next area, Shift+F6 to go back.', autocapitalize: 'off', autocomplete: 'off', autocorrect: 'off', spellcheck: 'false', rows: '1' });
  const surface = h('div', { class: 'bp-surface', role: 'application', 'aria-label': 'Sandbox Chrome page' }, canvas);
  const agentCursor = h('div', { class: 'bp-agent', hidden: true }, h('span', { class: 'bp-agent-arrow' }), h('span', { class: 'bp-agent-label' }));
  const agentPill = h('div', { class: 'bp-agent-pill', hidden: true });
  const copied = h('div', { class: 'bp-copied', role: 'status', hidden: true }, 'Copied');
  const sharedSize = h('div', { class: 'bp-shared', hidden: true },
    h('span', null, 'Sized for another viewer'),
    h('button', { type: 'button', class: 'btn btn-sm', onClick: () => { sentViewport = null; sendViewport(true); } }, 'Fit my panel'));
  const overlay = h('div', { class: 'bp-overlay' }, 'Connecting to the sandbox Chrome…');
  const el = h('div', { class: 'bp-view bp-stream' }, surface, keys, agentCursor, agentPill, copied, sharedSize, overlay);
  const viewBadge = h('span', { class: 'chip bp-viewchip', hidden: true });
  const keyboardButton = h('button', { type: 'button', class: 'btn btn-sm bp-keyboard', title: 'Show the keyboard', onClick: () => keys.focus() }, 'Keyboard');

  function send(message) {
    return socket ? socket.send(message) : false;
  }

  function watchedTab() {
    return tabs.find((t) => t.id === watched) || null;
  }

  function showOverlay(content) {
    overlay.hidden = false;
    overlay.replaceChildren();
    if (content instanceof Node) overlay.append(content); else overlay.textContent = content;
  }

  function updateOverlay() {
    const tab = watchedTab();
    if (!watched) showOverlay('');
    else if (!tab) showOverlay('This tab of the sandbox Chrome is closed.');
    else if (hasPicture) overlay.hidden = true;
    else showOverlay(skeleton('Loading page…', 3, { block: true }));
  }

  function updateBadge() {
    const pixels = streamMode === 'pixels';
    viewBadge.hidden = !pixels || !watched;
    viewBadge.textContent = 'JPEG mode';
    viewBadge.title = streamReason ? `Lower frame rate: ${streamReason}` : 'Lower frame rate: JPEG frames';
  }

  // ---------------------------------------------------------------- video
  function closeDecoder() {
    if (decoder && decoder.state !== 'closed') {
      try { decoder.close(); } catch { /* already closed */ }
    }
    decoder = null;
    dropPicture();
  }

  function dropPicture() {
    if (pendingPicture) { try { pendingPicture.close(); } catch { /* closed */ } }
    pendingPicture = null;
    if (pictureFrame) cancelAnimationFrame(pictureFrame);
    pictureFrame = 0;
  }

  /** Asks the server for a keyframe (rate-limited): the stream has none on a timer. */
  function askKeyframe() {
    const now = performance.now();
    if (now - keyframeAsked < KEYFRAME_REQUEST_MS) return;
    if (send({ type: 'keyframe' })) keyframeAsked = now;
  }

  function configure(message) {
    const description = message.description ? base64Bytes(message.description) : null;
    decoderConfig = { codec: message.codec, optimizeForLatency: true, codedWidth: message.width, codedHeight: message.height };
    if (description) decoderConfig.description = description;
    setCanvasSize(message.width, message.height);
    startDecoder();
  }

  function startDecoder() {
    closeDecoder();
    waitingKey = true;
    if (!decoderConfig || typeof VideoDecoder !== 'function') return;
    try {
      decoder = new VideoDecoder({ output: drawVideo, error: onDecoderError });
      decoder.configure(decoderConfig);
    } catch (error) {
      console.warn('video decoder unavailable; using JPEG frames', error);
      decoder = null;
      setMode('pixels', 'this browser cannot decode the stream');
    }
  }

  function onDecoderError(error) {
    if (error && error.name === 'NotSupportedError') {
      console.warn('this browser cannot decode the stream; using JPEG frames', error);
      closeDecoder();
      setMode('pixels', `this browser cannot decode ${decoderConfig ? decoderConfig.codec : 'the video'}`);
      return;
    }
    console.warn('video decoding failed; asking for a keyframe', error);
    // A new decoder resumes at the next keyframe.
    setTimeout(() => { startDecoder(); askKeyframe(); }, 0);
  }

  function decodeVideo(frame) {
    if (!decoder || decoder.state !== 'configured' || streamMode !== 'video') return;
    if (waitingKey && !frame.keyframe) { askKeyframe(); return; }
    if (!frame.keyframe && decoder.decodeQueueSize > MAX_DECODE_BACKLOG) {
      // This device cannot keep up at all: start over at a fresh keyframe.
      startDecoder();
      askKeyframe();
      return;
    }
    waitingKey = false;
    try {
      decoder.decode(new EncodedVideoChunk({ type: frame.keyframe ? 'key' : 'delta', timestamp: frame.timestamp, data: frame.data }));
    } catch (error) {
      onDecoderError(error);
    }
  }

  /** A decoded picture: drawn at once when nothing else is being decoded (lowest latency),
   *  else kept as the newest and drawn on the next animation frame, so a burst costs one draw. */
  function drawVideo(frame) {
    if (pendingPicture) pendingPicture.close();
    pendingPicture = frame;
    if (decoder && decoder.decodeQueueSize > 0) {
      if (!pictureFrame) pictureFrame = requestAnimationFrame(drawPicture);
      return;
    }
    drawPicture();
  }

  function drawPicture() {
    if (pictureFrame) cancelAnimationFrame(pictureFrame);
    pictureFrame = 0;
    const frame = pendingPicture;
    pendingPicture = null;
    if (!frame) return;
    try {
      if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) setCanvasSize(frame.displayWidth, frame.displayHeight);
      g.drawImage(frame, 0, 0);
    } finally {
      frame.close();
    }
    picture();
  }

  function picture() {
    if (!hasPicture) {
      hasPicture = true;
      updateOverlay();
    }
  }

  // ---------------------------------------------------------------- JPEG fallback
  async function decodeJpeg(frame) {
    if (jpegDecoding) { jpegQueued = frame; return; } // keep only the newest pending frame
    jpegDecoding = true;
    try {
      if (frame.tabId === watched && streamMode === 'pixels') {
        const bitmap = await createImageBitmap(new Blob([frame.bytes], { type: 'image/jpeg' }));
        if (canvas.width !== bitmap.width || canvas.height !== bitmap.height) setCanvasSize(bitmap.width, bitmap.height);
        g.drawImage(bitmap, 0, 0);
        bitmap.close();
        picture();
      }
    } catch { /* corrupt frame: skip */ }
    jpegDecoding = false;
    if (jpegQueued) { const next = jpegQueued; jpegQueued = null; decodeJpeg(next); }
  }

  function setMode(next, reason) {
    mode = next;
    if (next === 'pixels') { streamReason = reason || streamReason; closeDecoder(); }
    send({ type: 'view', mode: next });
    updateBadge();
  }

  // ---------------------------------------------------------------- layout
  function setCanvasSize(width, height) {
    if (!width || !height) return;
    canvas.width = width;
    canvas.height = height;
    layout();
  }

  /** Scales the page to the panel: the shared viewport (CSS px) shown at most at its size. */
  function layout() {
    const rect = el.getBoundingClientRect();
    const vw = serverViewport ? serverViewport.width : rect.width;
    const vh = serverViewport ? serverViewport.height : rect.height;
    if (!vw || !vh || !rect.width || !rect.height) return;
    fit = Math.min(1, rect.width / vw, rect.height / vh);
    canvas.style.width = `${Math.round(vw * fit)}px`;
    canvas.style.height = `${Math.round(vh * fit)}px`;
    surface.style.width = canvas.style.width;
    surface.style.height = canvas.style.height;
    const mine = sentViewport && serverViewport && sentViewport.width === serverViewport.width && sentViewport.height === serverViewport.height;
    const larger = serverViewport && (serverViewport.width > Math.round(rect.width) + 1 || serverViewport.height > Math.round(rect.height) + 1);
    // While this panel resizes (its new size not yet sent or answered) the page is only scaled
    // for a moment: that is not another viewer's size.
    const resizing = awaitingViewport || !sentViewport || sentViewport.width !== Math.round(rect.width) || sentViewport.height !== Math.round(rect.height);
    sharedSize.hidden = Boolean(mine || resizing || !serverViewport || !larger);
  }

  function sendViewport(force = false) {
    const rect = el.getBoundingClientRect();
    const width = Math.round(rect.width);
    const height = Math.round(rect.height);
    if (width < 50 || height < 50) return;
    if (!force && sentViewport && sentViewport.width === width && sentViewport.height === height) return;
    if (send({ type: 'viewport', width, height })) {
      sentViewport = { width, height };
      awaitingViewport = true;
      clearTimeout(awaitingTimer);
      awaitingTimer = setTimeout(() => { awaitingViewport = false; layout(); }, 1000);
    }
  }

  const observer = new ResizeObserver(() => {
    layout();
    clearTimeout(viewportTimer);
    viewportTimer = setTimeout(sendViewport, 150);
  });
  observer.observe(el);

  // ---------------------------------------------------------------- pointer
  /** CSS px of the page viewport under a pointer event, or null outside it. */
  function point(event) {
    const rect = canvas.getBoundingClientRect();
    if (!rect.width || !rect.height) return null;
    const x = (event.clientX - rect.left) / fit;
    const y = (event.clientY - rect.top) / fit;
    const vw = serverViewport ? serverViewport.width : rect.width / fit;
    const vh = serverViewport ? serverViewport.height : rect.height / fit;
    if (x < 0 || y < 0 || x > vw || y > vh) return null;
    return { x: Math.round(x * 10) / 10, y: Math.round(y * 10) / 10 };
  }

  function mouse(event, kind, p, button = 'none', extra = {}) {
    if (!watched || !p) return;
    send({ type: 'mouse', tab_id: watched, event: kind, x: p.x, y: p.y, button, ...extra });
  }

  /** Sends the newest pointer position, at most once per animation frame and only when it
   *  changed (the server keeps only the newest anyway). */
  function flushMove() {
    moveFrame = 0;
    const move = pendingMove;
    pendingMove = null;
    if (!move) return;
    const p = point(move);
    if (!p || (lastMove && lastMove.x === p.x && lastMove.y === p.y)) return;
    lastMove = p;
    mouse(move, 'move', p);
  }

  surface.addEventListener('pointerdown', (event) => {
    if (event.pointerType === 'touch') { touchStart(event); return; }
    event.preventDefault();
    keys.focus({ preventScroll: true });
    try { surface.setPointerCapture(event.pointerId); } catch { /* not capturable */ }
    if (pendingMove) { cancelAnimationFrame(moveFrame); flushMove(); }
    lastMove = null;
    pressed |= 1 << event.button;
    mouse(event, 'down', point(event), MOUSE_BUTTONS[event.button] || 'left');
  });
  surface.addEventListener('pointermove', (event) => {
    if (event.pointerType === 'touch') { touchMove(event); return; }
    pendingMove = event;
    if (!moveFrame) moveFrame = requestAnimationFrame(flushMove);
  });
  surface.addEventListener('pointerup', (event) => {
    if (event.pointerType === 'touch') { touchEnd(event); return; }
    if (pendingMove) { cancelAnimationFrame(moveFrame); flushMove(); }
    pressed &= ~(1 << event.button);
    // Released outside the page (captured): report it at the nearest edge so nothing stays held.
    const p = point(event) || clampedPoint(event);
    mouse(event, 'up', p, MOUSE_BUTTONS[event.button] || 'left');
  });
  surface.addEventListener('pointercancel', (event) => { if (event.pointerType === 'touch') touch = null; });
  surface.addEventListener('contextmenu', (event) => event.preventDefault());
  surface.addEventListener('wheel', (event) => {
    event.preventDefault();
    const p = point(event);
    if (!p) return;
    const scale = event.deltaMode === 1 ? 40 : event.deltaMode === 2 ? (serverViewport ? serverViewport.height : 800) : 1;
    wheel.dx += event.deltaX * scale;
    wheel.dy += event.deltaY * scale;
    wheel.x = p.x;
    wheel.y = p.y;
    if (!wheelFrame) wheelFrame = requestAnimationFrame(flushWheel);
  }, { passive: false });

  function flushWheel() {
    wheelFrame = 0;
    const { dx, dy, x, y } = wheel;
    wheel = { dx: 0, dy: 0, x, y };
    if (!dx && !dy) return;
    mouse(null, 'wheel', { x, y }, 'none', { dx: Math.round(dx * 10) / 10, dy: Math.round(dy * 10) / 10 });
  }

  function clampedPoint(event) {
    const rect = canvas.getBoundingClientRect();
    const vw = serverViewport ? serverViewport.width : rect.width / fit;
    const vh = serverViewport ? serverViewport.height : rect.height / fit;
    const x = Math.min(Math.max((event.clientX - rect.left) / fit, 0), vw - 1);
    const y = Math.min(Math.max((event.clientY - rect.top) / fit, 0), vh - 1);
    return { x: Math.round(x), y: Math.round(y) };
  }

  // Touch: a tap clicks, a drag scrolls the page under the finger.
  function touchStart(event) {
    if (touch) return; // one finger at a time
    touch = { id: event.pointerId, x: event.clientX, y: event.clientY, lastX: event.clientX, lastY: event.clientY, scrolling: false };
  }
  function touchMove(event) {
    if (!touch || touch.id !== event.pointerId) return;
    event.preventDefault();
    if (!touch.scrolling && Math.hypot(event.clientX - touch.x, event.clientY - touch.y) > TAP_SLOP) touch.scrolling = true;
    if (!touch.scrolling) return;
    const p = point({ clientX: touch.x, clientY: touch.y }) || clampedPoint(event);
    wheel.dx += (touch.lastX - event.clientX) / fit;
    wheel.dy += (touch.lastY - event.clientY) / fit;
    wheel.x = p.x;
    wheel.y = p.y;
    touch.lastX = event.clientX;
    touch.lastY = event.clientY;
    if (!wheelFrame) wheelFrame = requestAnimationFrame(flushWheel);
  }
  function touchEnd(event) {
    if (!touch || touch.id !== event.pointerId) return;
    const tap = !touch.scrolling;
    touch = null;
    if (!tap) return;
    const p = point(event);
    mouse(event, 'move', p);
    mouse(event, 'down', p, 'left');
    mouse(event, 'up', p, 'left');
  }

  // ---------------------------------------------------------------- keys
  function modifiers(event) {
    return (event.altKey ? 1 : 0) | (event.ctrlKey ? 2 : 0) | (event.metaKey ? 4 : 0) | (event.shiftKey ? 8 : 0);
  }

  function key(kind, k, code, mods = 0) {
    if (!watched) return;
    if (kind === 'down') held.set(code || k, k); else held.delete(code || k);
    send({ type: 'key', tab_id: watched, event: kind, key: k, code, modifiers: mods });
  }

  /** Lifts every key this view holds down (the page lost the keyboard mid-press). */
  function releaseKeys() {
    for (const [code, k] of [...held]) key('up', k, code);
  }

  function text(value) {
    if (!watched || !value) return;
    // Bounded like the server (32 KiB of text per message).
    for (let i = 0; i < value.length; i += 8000) send({ type: 'key', tab_id: watched, event: 'char', text: value.slice(i, i + 8000) });
  }

  /** ⌘ on macOS: editing shortcuts become Ctrl+key, ⌘ arrows become Home/End. */
  function macCommand(event) {
    const lower = event.key.toLowerCase();
    const arrows = { ArrowLeft: ['Home', 'Home'], ArrowRight: ['End', 'End'], ArrowUp: ['Home', 'Home', true], ArrowDown: ['End', 'End', true] };
    if (arrows[event.key]) {
      const [k, code, ctrl] = arrows[event.key];
      if (ctrl) key('down', 'Control', 'ControlLeft');
      key('down', k, code);
      key('up', k, code);
      if (ctrl) key('up', 'Control', 'ControlLeft');
      return true;
    }
    if (!MAC_EDIT_KEYS.has(lower)) return false;
    if (lower === 'c' || lower === 'x') requestCopy();
    key('down', 'Control', 'ControlLeft');
    key('down', event.key.length === 1 ? lower : event.key, event.code);
    key('up', event.key.length === 1 ? lower : event.key, event.code);
    key('up', 'Control', 'ControlLeft');
    return true;
  }

  keys.addEventListener('keydown', (event) => {
    if (event.key === 'F6' && !event.altKey && !event.metaKey && !event.ctrlKey) {
      event.preventDefault();
      focusNextRegion(event.shiftKey);
      return;
    }
    if (event.isComposing || event.key === 'Process' || event.keyCode === 229) return; // IME
    const lower = event.key.toLowerCase();
    const command = IS_MAC ? event.metaKey : event.ctrlKey;
    if (command && OWN_SHORTCUTS.has(lower)) return; // the user's own browser
    if (command && lower === 'v') return; // the paste event sends the user's clipboard
    if (IS_MAC && event.metaKey) {
      if (event.key !== 'Meta' && macCommand(event)) event.preventDefault();
      return;
    }
    if (event.key === 'Dead') return; // the composed character arrives as input
    if (!IS_MAC && event.ctrlKey && (lower === 'c' || lower === 'x') && !event.altKey) requestCopy();
    // AltGraph characters are text (their Ctrl+Alt is not the page's business).
    if (event.key.length === 1 && event.getModifierState && event.getModifierState('AltGraph')) {
      event.preventDefault();
      text(event.key);
      return;
    }
    event.preventDefault();
    key('down', event.key, event.code, modifiers(event));
  });
  keys.addEventListener('keyup', (event) => {
    if (event.key === 'F6' || event.key === 'Dead' || event.key === 'Process' || event.isComposing) return;
    if (IS_MAC && (event.metaKey || event.key === 'Meta')) return;
    const lower = event.key.toLowerCase();
    if ((IS_MAC ? event.metaKey : event.ctrlKey) && (OWN_SHORTCUTS.has(lower) || lower === 'v')) return;
    key('up', event.key, event.code, modifiers(event));
  });
  keys.addEventListener('compositionend', (event) => {
    compositionText = event.data || '';
    text(compositionText);
    keys.value = '';
    setTimeout(() => { compositionText = null; }, 100);
  });
  keys.addEventListener('beforeinput', (event) => {
    // Phone keyboards send no usable key events: their edits arrive here.
    if (event.inputType === 'deleteContentBackward' && !event.isComposing) {
      event.preventDefault();
      key('down', 'Backspace', 'Backspace');
      key('up', 'Backspace', 'Backspace');
    } else if ((event.inputType === 'insertLineBreak' || event.inputType === 'insertParagraph') && !event.isComposing) {
      event.preventDefault();
      key('down', 'Enter', 'Enter');
      key('up', 'Enter', 'Enter');
    }
  });
  keys.addEventListener('input', (event) => {
    if (event.isComposing) return;
    const value = event.data || keys.value;
    if (value && value !== compositionText && /^insert/.test(event.inputType || 'insertText')) text(value);
    keys.value = '';
  });
  keys.addEventListener('paste', (event) => {
    event.preventDefault();
    const value = event.clipboardData ? event.clipboardData.getData('text/plain') : '';
    text(value.slice(0, 32000));
  });
  // Keys held when the page loses the keyboard would stay down on the server.
  keys.addEventListener('blur', releaseKeys);

  /** Asks for the page's selection; the answer goes to the user's clipboard. In the key's own
   *  event (a user gesture) the clipboard is claimed with a pending item where supported
   *  (Safari needs this). */
  function requestCopy() {
    if (!watched) return;
    if (pendingCopy) { clearTimeout(pendingCopy.timer); pendingCopy.reject(new Error('superseded')); }
    let resolve;
    let reject;
    const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
    promise.catch(() => {});
    pendingCopy = { resolve, reject, timer: setTimeout(() => { if (pendingCopy) pendingCopy.reject(new Error('no answer')); pendingCopy = null; }, 3000), claimed: false };
    if (window.ClipboardItem && navigator.clipboard && navigator.clipboard.write && IS_MAC) {
      try {
        const item = new ClipboardItem({ 'text/plain': promise.then((value) => new Blob([value], { type: 'text/plain' })) });
        navigator.clipboard.write([item]).then(showCopied, () => {});
        pendingCopy.claimed = true;
      } catch { /* fall back to writeText on the answer */ }
    }
    send({ type: 'copy', tab_id: watched });
  }

  function onClipboard(value) {
    const pending = pendingCopy;
    pendingCopy = null;
    if (pending) clearTimeout(pending.timer);
    if (!value) { if (pending) pending.reject(new Error('empty')); return; }
    if (pending && pending.claimed) { pending.resolve(value); return; }
    if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(value).then(showCopied, () => toast('The browser blocked the clipboard; copy again with the page focused.', { kind: 'warn' }));
  }

  function showCopied() {
    copied.hidden = false;
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => { copied.hidden = true; }, 1200);
  }

  // ---------------------------------------------------------------- agent cursor (BRW-007)
  function onAgent(message) {
    if (hooks.onAgent) hooks.onAgent(message);
    if (!watched || message.tab_id !== watched) return;
    const label = `${message.label || 'Agent'}${message.action === 'type' ? ' · typing' : ''}`;
    if (message.x != null && message.y != null) lastAgent = { x: message.x, y: message.y };
    if (lastAgent && (message.action === 'click' || message.action === 'type')) {
      const offset = canvas.getBoundingClientRect();
      const base = el.getBoundingClientRect();
      agentCursor.hidden = false;
      agentCursor.style.left = `${Math.round(offset.left - base.left + lastAgent.x * fit)}px`;
      agentCursor.style.top = `${Math.round(offset.top - base.top + lastAgent.y * fit)}px`;
      agentCursor.querySelector('.bp-agent-label').textContent = label;
      if (message.action === 'click') {
        const ripple = h('span', { class: 'bp-agent-ripple' });
        agentCursor.append(ripple);
        setTimeout(() => ripple.remove(), 700);
      }
    } else {
      agentPill.hidden = false;
      agentPill.textContent = `${message.label || 'Agent'} ${message.action === 'open' ? 'opened a tab' : 'navigated'}`;
    }
    clearTimeout(agentTimer);
    agentTimer = setTimeout(() => { agentCursor.hidden = true; agentPill.hidden = true; }, AGENT_CURSOR_IDLE_MS);
  }

  // ---------------------------------------------------------------- socket
  function onTabs(list) {
    tabs = list;
    const fresh = tabs.filter((t) => !knownIds.has(t.id));
    knownIds = new Set(tabs.map((t) => t.id));
    if (hooks.onTabs) hooks.onTabs(tabs);
    if (expectNew > 0 && fresh.length) {
      expectNew -= 1;
      if (hooks.onCreated) hooks.onCreated(fresh[fresh.length - 1].id);
    }
    updateOverlay();
  }

  function onMessage(data) {
    if (typeof data !== 'string') {
      const video = parseVideoFrame(data);
      if (video) {
        // Acknowledged on arrival: the server paces the stream by these (flow control).
        framesReceived += 1;
        send({ type: 'ack', frames: framesReceived });
        decodeVideo(video);
        return;
      }
      const frame = parseBrowserFrame(data);
      if (frame) decodeJpeg(frame);
      return;
    }
    let message;
    try { message = JSON.parse(data); } catch { return; }
    switch (message.type) {
      case 'tabs':
        if (Array.isArray(message.tabs)) onTabs(message.tabs);
        break;
      case 'stream':
        streamMode = message.mode;
        streamReason = message.reason || (mode === 'pixels' ? streamReason : null);
        if (streamMode === 'pixels') closeDecoder();
        updateBadge();
        break;
      case 'video_config':
        if (streamMode !== 'pixels') configure(message);
        break;
      case 'viewport':
        serverViewport = { width: message.width, height: message.height };
        awaitingViewport = false;
        clearTimeout(awaitingTimer);
        layout();
        break;
      case 'clipboard':
        onClipboard(typeof message.text === 'string' ? message.text : '');
        break;
      case 'agent_activity':
        onAgent(message);
        break;
      case 'action_requested':
        if (hooks.onActionRequested) hooks.onActionRequested(message);
        break;
      case 'error':
        lastError = { code: message.code, message: message.message || '' };
        // `upstream` precedes a 1011 close when Chromium is missing or the browser stopped (the
        // overlay explains it); a tab that vanished is replaced by the next tab list.
        if (message.code !== 'upstream' && message.code !== 'not_found') toast(message.message || 'Browser error', { kind: 'error', detail: message.code ? `code: ${message.code}` : null });
        break;
      default:
        break;
    }
  }

  function showFailure(textContent) {
    showOverlay(h('div', { class: 'stack overlay-message' },
      h('p', null, textContent),
      h('button', { type: 'button', class: 'btn btn-sm', onClick: () => { lastError = null; if (socket) socket.retry(); else connect(); } }, 'Retry')));
  }

  function connect() {
    if (socket) return;
    socket = new ReconnectingSocket({
      transport: ctx.transport,
      path: `/api/sandboxes/${enc(sandbox.id)}/browser`,
      onOpen: () => {
        sentViewport = null;
        hasPicture = false;
        streamMode = null;
        framesReceived = 0;
        keyframeAsked = 0;
        lastMove = null;
        if (mode === 'video' && !hasWebCodecs()) {
          mode = 'pixels';
          streamReason = 'this browser cannot decode video (no WebCodecs)';
        }
        if (mode === 'pixels') send({ type: 'view', mode: 'pixels' });
        // The server streams the shown (active) tab; nothing is brought forward on (re)connect.
        sendViewport(true);
        updateOverlay();
        if (hooks.onConnected) hooks.onConnected();
      },
      onMessage,
      onStatus: (status, detail = {}) => {
        if (status === 'reconnecting' && detail.code === CLOSE_SERVER_ERROR) {
          // The browser failed to start or stopped (e.g. Chromium missing): do not hammer it.
          const reason = lastError && lastError.message ? lastError.message : 'The sandbox Chrome stopped.';
          if (socket) socket.stop();
          showFailure(`The sandbox Chrome is not available: ${reason}`);
        } else if (status === 'reconnecting' || status === 'connecting') {
          showOverlay(status === 'connecting' ? 'Connecting to the sandbox Chrome…' : 'Reconnecting to the sandbox Chrome…');
        } else if (status === 'failed') {
          showFailure(detail.code === CLOSE_REVOKED ? 'Your access to this sandbox browser was revoked.' : 'The sandbox Chrome is unreachable.');
        }
      },
    });
    socket.start();
  }

  return {
    el,
    viewBadge,
    keyboardButton,
    get tabs() { return tabs; },
    get watched() { return watched; },
    get isOpen() { return Boolean(socket && socket.isOpen); },
    get streamMode() { return streamMode; },
    /** Show `tabId`. The display shows one tab: with `activate` (the user chose it) it is brought
     *  to the front for every viewer; without, the panel follows the tab the server shows. */
    watch(tabId, { activate = true } = {}) {
      const changed = watched !== tabId;
      watched = tabId || null;
      if (changed) {
        agentCursor.hidden = true;
        agentPill.hidden = true;
        lastAgent = null;
      }
      if (activate && watched) send({ type: 'watch', tab_id: watched });
      updateOverlay();
      updateBadge();
    },
    /** Opens a tab; `onCreated` names it once it exists. */
    open(url) {
      expectNew += 1;
      if (!send({ type: 'open', url })) expectNew -= 1;
    },
    navigate(tabId, url) { send({ type: 'navigate', tab_id: tabId, url }); },
    history(tabId, delta) { send({ type: 'history', tab_id: tabId, delta }); },
    reload(tabId) { send({ type: 'reload', tab_id: tabId }); },
    close(tabId) { send({ type: 'close', tab_id: tabId }); },
    focus() { keys.focus({ preventScroll: true }); },
    layout,
    start() { connect(); setTimeout(() => { sendViewport(); layout(); }, 0); },
    stop() {
      if (socket) socket.stop();
      socket = null;
      closeDecoder();
      hasPicture = false;
    },
    destroy() {
      observer.disconnect();
      clearTimeout(viewportTimer);
      clearTimeout(awaitingTimer);
      clearTimeout(agentTimer);
      clearTimeout(copiedTimer);
      cancelAnimationFrame(moveFrame);
      cancelAnimationFrame(wheelFrame);
      if (socket) socket.stop();
      socket = null;
      closeDecoder();
    },
  };
}
