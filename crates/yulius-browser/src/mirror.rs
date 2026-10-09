//! DOM mirroring of tabs (SPEC-001 BRW-005, SPEC-002 ADR-09).
//!
//! The rrweb recorder (`assets/rrweb-record.cjs`, MIT) is injected through CDP into the top
//! frame of every document of a watched tab. It batches rrweb events (JSON) to the CDP binding
//! [`BINDING`]; the instance keeps, per tab, the events since the last full snapshot so a new
//! viewer can rebuild the page, and forwards new batches to subscribed viewers. A `Meta` event
//! (type 4) starts a new stream (navigation or checkout). Bounds: one binding call carries at
//! most [`MAX_BATCH_BYTES`]; a buffer beyond [`MAX_BUFFER_BYTES`] or [`MAX_BUFFER_EVENTS`] asks
//! the recorder for a fresh full snapshot (checkout), which replaces it.

use std::sync::Arc;

use serde_json::Value;

/// Name of the CDP binding the recorder calls with a JSON array of events.
pub const BINDING: &str = "__yuliusMirrorSend";
/// Largest accepted binding payload.
pub const MAX_BATCH_BYTES: usize = 8 << 20;
/// Buffered bytes per tab before a checkout is requested.
pub const MAX_BUFFER_BYTES: usize = 24 << 20;
/// Buffered events per tab before a checkout is requested.
pub const MAX_BUFFER_EVENTS: usize = 20_000;
/// rrweb `EventType.Meta`.
const META: u64 = 4;
/// rrweb `EventType.FullSnapshot`.
const FULL_SNAPSHOT: u64 = 2;

const RECORD_SOURCE: &str = include_str!("../assets/rrweb-record.cjs");

/// The expression run in every document: starts the recorder once, in the top frame only.
#[must_use]
pub fn injection_script() -> String {
    format!(
        r#"(() => {{
  const g = globalThis;
  if (g.top !== g || (g.__yuliusMirror && g.__yuliusMirror.active)) return;
  if (typeof g.{BINDING} !== 'function') return;
  const send = g.{BINDING};
  const module = {{ exports: {{}} }};
  (function (module, exports) {{
{RECORD_SOURCE}
  }})(module, module.exports);
  const record = module.exports.record;
  let queue = [];
  let size = 0;
  let timer = 0;
  const flush = () => {{
    timer = 0;
    if (!queue.length) return;
    const batch = '[' + queue.join(',') + ']';
    queue = [];
    size = 0;
    try {{ send(batch); }} catch (_) {{}}
  }};
  const options = {{
    emit(event) {{
      let text;
      try {{ text = JSON.stringify(event); }} catch (_) {{ return; }}
      queue.push(text);
      size += text.length;
      if (size > 4000000) flush();
      else if (!timer) timer = setTimeout(flush, 40);
    }},
    recordCanvas: true,
    // Inputs are recorded on every change so typed text streams into the mirror at once.
    sampling: {{ canvas: 2, mousemove: false, mouseInteraction: false, scroll: 100, media: 800 }},
    dataURLOptions: {{ type: 'image/webp', quality: 0.7 }},
    inlineStylesheet: true,
    collectFonts: true,
    maskAllInputs: false,
    slimDOMOptions: {{ script: true, comment: true }},
  }};
  let stop = null;
  try {{
    stop = record(options);
    // record() returns nothing when it failed, e.g. when the page's CSP refuses the canvas
    // sampling worker: record without canvases then.
    if (typeof stop !== 'function') stop = record({{ ...options, recordCanvas: false }});
  }} catch (_) {{ return; }}
  if (typeof stop !== 'function') return;
  g.__yuliusMirror = {{
    active: true,
    snapshot() {{ try {{ record.takeFullSnapshot(true); }} catch (_) {{}} flush(); }},
    stop() {{ try {{ if (stop) stop(); }} catch (_) {{}} g.__yuliusMirror = {{ active: false }}; }},
  }};
}})();"#
    )
}

/// Expression asking a running recorder for a fresh full snapshot.
pub const SNAPSHOT_EXPRESSION: &str = "globalThis.__yuliusMirror && globalThis.__yuliusMirror.active && globalThis.__yuliusMirror.snapshot()";
/// Expression stopping a running recorder.
pub const STOP_EXPRESSION: &str = "globalThis.__yuliusMirror && globalThis.__yuliusMirror.active && globalThis.__yuliusMirror.stop()";

/// Events for viewers. `reset` starts a new stream: the viewer rebuilds its mirror from these
/// events (a `Meta` and a full snapshot first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomBatch {
    /// The events replace the viewer's current stream.
    pub reset: bool,
    /// rrweb events as JSON texts.
    pub events: Arc<[Arc<str>]>,
}

/// Why a payload was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MirrorError {
    /// Larger than [`MAX_BATCH_BYTES`].
    #[error("mirror batch exceeds {MAX_BATCH_BYTES} bytes")]
    TooLarge,
    /// Not a JSON array of objects.
    #[error("mirror batch is not a JSON array of events")]
    Malformed,
}

/// Events of one tab since its last `Meta`.
#[derive(Debug, Default)]
pub struct MirrorBuffer {
    events: Vec<Arc<str>>,
    bytes: usize,
    has_snapshot: bool,
}

impl MirrorBuffer {
    /// Adds a recorder payload. Returns the batch for viewers and whether a checkout (fresh
    /// full snapshot) should be requested because the buffer outgrew its bounds.
    ///
    /// # Errors
    /// [`MirrorError`] for oversized or malformed payloads (nothing is buffered).
    pub fn push(&mut self, payload: &str) -> Result<(DomBatch, bool), MirrorError> {
        if payload.len() > MAX_BATCH_BYTES {
            return Err(MirrorError::TooLarge);
        }
        let parsed: Vec<Value> =
            serde_json::from_str(payload).map_err(|_| MirrorError::Malformed)?;
        let mut reset = false;
        let mut batch = Vec::with_capacity(parsed.len());
        for event in parsed {
            let kind = event
                .get("type")
                .and_then(Value::as_u64)
                .ok_or(MirrorError::Malformed)?;
            if kind == META {
                // A new document or checkout: earlier events are obsolete for new viewers, and
                // this batch replaces what viewers show.
                self.events.clear();
                self.bytes = 0;
                self.has_snapshot = false;
                batch.clear();
                reset = true;
            }
            if kind == FULL_SNAPSHOT {
                self.has_snapshot = true;
            }
            let text: Arc<str> = Arc::from(event.to_string());
            self.bytes += text.len();
            self.events.push(Arc::clone(&text));
            batch.push(text);
        }
        let checkout = self.bytes > MAX_BUFFER_BYTES || self.events.len() > MAX_BUFFER_EVENTS;
        Ok((
            DomBatch {
                reset,
                events: batch.into(),
            },
            checkout,
        ))
    }

    /// Everything a new viewer needs, when a full snapshot is buffered.
    #[must_use]
    pub fn snapshot(&self) -> Option<DomBatch> {
        self.has_snapshot.then(|| DomBatch {
            reset: true,
            events: self.events.clone().into(),
        })
    }

    /// Forgets the buffered events (the recorder stopped).
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn a_meta_starts_a_new_stream_and_snapshots_replay_it() -> TestResult {
        let mut buffer = MirrorBuffer::default();
        let (first, _) = buffer.push(r#"[{"type":3,"data":{}}]"#)?;
        assert!(!first.reset);
        assert!(buffer.snapshot().is_none(), "no full snapshot yet");
        let (start, checkout) =
            buffer.push(r#"[{"type":4,"data":{"href":"http://x/"}},{"type":2,"data":{}}]"#)?;
        assert!(start.reset && !checkout);
        assert_eq!(start.events.len(), 2);
        buffer.push(r#"[{"type":3,"data":{"source":0}}]"#)?;
        let replay = buffer.snapshot().ok_or("snapshot")?;
        assert!(replay.reset);
        assert_eq!(replay.events.len(), 3, "meta, full snapshot, mutation");
        assert!(replay.events[0].contains("\"type\":4"));
        Ok(())
    }

    #[test]
    fn malformed_or_oversized_payloads_are_refused() {
        let mut buffer = MirrorBuffer::default();
        assert_eq!(buffer.push("{}"), Err(MirrorError::Malformed));
        assert_eq!(buffer.push(r#"[{"data":1}]"#), Err(MirrorError::Malformed));
        let big = format!("[{}]", "1".repeat(MAX_BATCH_BYTES));
        assert_eq!(buffer.push(&big), Err(MirrorError::TooLarge));
    }

    #[test]
    fn an_outgrown_buffer_asks_for_a_checkout() -> TestResult {
        let mut buffer = MirrorBuffer::default();
        buffer.push(r#"[{"type":4,"data":{}},{"type":2,"data":{}}]"#)?;
        let many = format!(
            "[{}]",
            vec![r#"{"type":3,"data":{}}"#; MAX_BUFFER_EVENTS].join(",")
        );
        let (_, checkout) = buffer.push(&many)?;
        assert!(checkout);
        Ok(())
    }

    #[test]
    fn the_injection_runs_once_in_the_top_frame_with_the_binding() {
        let script = injection_script();
        assert!(script.contains("g.top !== g"));
        assert!(script.contains(&format!("typeof g.{BINDING} !== 'function'")));
        assert!(
            script.contains("exports.record = record"),
            "the recorder is embedded"
        );
    }
}
