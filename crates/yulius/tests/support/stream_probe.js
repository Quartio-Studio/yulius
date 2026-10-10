// Client-side measurement of the streamed browser (tests/live_ui_stream.rs; F 2026-10-10).
// Injected into the Yulius UI before its scripts (CDP Page.addScriptToEvaluateOnNewDocument), it
// records on the client clock (performance.now()):
// - every binary video message received on the browser socket (time, bytes, keyframe, server µs)
//   and every text message;
// - every message the UI sends on it;
// - decode() calls and decoder outputs (decode time, queue depth);
// - every video frame drawn on the panel canvas, with the rows of stream_probe.html decoded from
//   the frame's luma (scrollY, pointer x/y, input counter, check row);
// - the user's input as the UI sees it, in page CSS pixels.
// `window.__perf.take()` returns and clears what was recorded.
(() => {
  const P = (window.__perf = { rx: [], tx: [], dec: new Map(), decQ: [], out: [], draws: [], text: [], acks: 0 });
  const t0 = performance.now();
  const now = () => performance.now();
  const NativeWS = window.WebSocket;
  class PerfWS extends NativeWS {
    constructor(url, protocols) {
      super(url, protocols);
      this.__browser = String(url).includes('/browser');
      if (this.__browser) {
        P.socket = this;
        this.addEventListener('message', (event) => {
          const t = now();
          const data = event.data;
          if (data instanceof ArrayBuffer) {
            const view = new DataView(data);
            const kind = view.getUint8(0);
            if (kind === 4 && data.byteLength >= 10) P.rx.push([t, data.byteLength, view.getUint8(1) & 1, Number(view.getBigUint64(2, false))]);
            else P.rx.push([t, data.byteLength, -1, 0]);
          } else if (typeof data === 'string') {
            P.text.push([t, data.length, data.slice(0, 160)]);
          }
        });
      }
    }
    send(data) {
      if (this.__browser) P.tx.push([now(), typeof data === 'string' ? data : `[${data.byteLength || 0} bytes]`]);
      return super.send(data);
    }
  }
  window.WebSocket = PerfWS;

  if (typeof VideoDecoder === 'function') {
    const NativeDecoder = VideoDecoder;
    window.VideoDecoder = class extends NativeDecoder {
      constructor(init) {
        const output = init.output;
        super({
          output: (frame) => {
            const t = now();
            const sent = P.dec.get(frame.timestamp);
            P.out.push([t, frame.timestamp, sent === undefined ? -1 : t - sent]);
            P.dec.delete(frame.timestamp);
            P.current = frame.timestamp;
            output(frame);
          },
          error: init.error,
        });
      }
      decode(chunk) {
        P.dec.set(chunk.timestamp, now());
        P.decQ.push([now(), this.decodeQueueSize, chunk.type === 'key' ? 1 : 0, chunk.byteLength]);
        if (P.dec.size > 2000) P.dec.clear();
        return super.decode(chunk);
      }
    };
  }

  // Probe rows (16 px blocks at the top left of the page, 24 bits each), read from the luma
  // plane of a clone of each drawn frame (asynchronously, so the panel's canvas is untouched).
  const RECT = { x: 0, y: 0, width: 384, height: 64 };
  function decodeProbe(bytes, layout) {
    const { offset, stride } = layout[0];
    const rows = [];
    for (let r = 0; r < 4; r++) {
      let value = 0;
      for (let b = 0; b < 24; b++) {
        const y = bytes[offset + (r * 16 + 8) * stride + (b * 16 + 8)];
        value = (value << 1) | (y > 128 ? 1 : 0);
      }
      rows.push(value >>> 0);
    }
    const ok = ((rows[0] ^ rows[1] ^ rows[2] ^ 0x5a5a5a) & 0xffffff) === rows[3];
    return { scroll: rows[0], px: rows[1] >> 12, py: rows[1] & 0xfff, inputs: rows[2], ok };
  }

  const nativeDraw = CanvasRenderingContext2D.prototype.drawImage;
  CanvasRenderingContext2D.prototype.drawImage = function (image, ...rest) {
    const isVideo = typeof VideoFrame === 'function' && image instanceof VideoFrame && this.canvas && this.canvas.classList && this.canvas.classList.contains('bp-canvas');
    let clone = null;
    if (isVideo && P.probe !== false) { try { clone = image.clone(); } catch { clone = null; } }
    const result = nativeDraw.call(this, image, ...rest);
    if (isVideo) {
      const entry = [now(), image.timestamp, -1, 0, 0, 0, 0];
      P.draws.push(entry);
      if (clone) {
        try {
          const size = clone.allocationSize({ rect: RECT });
          const bytes = new Uint8Array(size);
          clone.copyTo(bytes, { rect: RECT }).then((layout) => {
            const p = decodeProbe(bytes, layout);
            entry[2] = p.ok ? 1 : 0; entry[3] = p.scroll; entry[4] = p.px; entry[5] = p.py; entry[6] = p.inputs;
          }).catch(() => {}).finally(() => clone.close());
        } catch { clone.close(); }
      }
    }
    return result;
  };

  // Input as the UI sees it (capture phase, before its handlers), in page CSS px of the
  // remote viewport (the canvas has the video's size; it is scaled by `fit`).
  P.inputs = [];
  function pagePoint(event) {
    const canvas = document.querySelector('.bp-canvas');
    if (!canvas) return [0, 0];
    const rect = canvas.getBoundingClientRect();
    const fit = rect.width / canvas.width;
    return [Math.round((event.clientX - rect.left) / fit), Math.round((event.clientY - rect.top) / fit)];
  }
  addEventListener('wheel', (e) => { P.inputs.push([now(), 'wheel', e.deltaY, 0]); }, { capture: true, passive: true });
  addEventListener('pointermove', (e) => { const [x, y] = pagePoint(e); P.inputs.push([now(), 'move', x, y]); }, { capture: true, passive: true });
  addEventListener('pointerdown', (e) => { const [x, y] = pagePoint(e); P.inputs.push([now(), 'down', x, y]); }, { capture: true, passive: true });
  addEventListener('keydown', (e) => { P.inputs.push([now(), 'key', e.key, 0]); }, { capture: true, passive: true });

  // A drain the harness calls: returns and clears what was recorded.
  P.take = () => {
    const out = { rx: P.rx, tx: P.tx, decQ: P.decQ, out: P.out, draws: P.draws, text: P.text, inputs: P.inputs };
    P.rx = []; P.tx = []; P.decQ = []; P.out = []; P.draws = []; P.text = []; P.inputs = [];
    return out;
  };
  P.t0 = t0;
})();
