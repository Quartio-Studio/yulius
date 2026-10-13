// HTTP API client and transport (docs/api.md).
// A transport is {kind, fetch(url, init) → Promise<Response>, openSocket(path) → WebSocket-like}.
// The real transport uses window.fetch / WebSocket; ?mock=1 swaps in assets/mock.js, so
// request building, error parsing and the WebSocket protocol code run unchanged in both.

import { BRAND } from './brand.js';

/** Error carrying the server's `{"error": {"code", "message"}}` and HTTP status. */
export class ApiError extends Error {
  constructor(status, code, message) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
  }
}

const REQUEST_TIMEOUT_MS = 30000;

// Fallback codes when a response has no `{"error"}` body (docs/api.md "Conventions").
const STATUS_CODES = { 400: 'invalid', 401: 'unauthenticated', 403: 'forbidden', 404: 'not_found', 409: 'conflict', 413: 'invalid', 422: 'invalid', 429: 'capacity', 502: 'upstream', 503: 'unavailable' };

/** Statuses whose responses have no body (a Response refuses one). */
const NULL_BODY = new Set([101, 204, 205, 304]);

/**
 * Sends a request with XMLHttpRequest, which reports upload progress (fetch does not), and
 * answers like fetch: a Response, a TypeError for network failures, an AbortError on abort.
 */
function xhrUpload(url, init, onProgress) {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open(init.method, url);
    for (const [name, value] of Object.entries(init.headers || {})) xhr.setRequestHeader(name, value);
    xhr.upload.addEventListener('progress', (event) => { if (event.lengthComputable) onProgress(event.loaded, event.total); });
    xhr.addEventListener('load', () => {
      const status = xhr.status;
      resolve(new Response(NULL_BODY.has(status) ? null : xhr.responseText, { status, headers: { 'Content-Type': xhr.getResponseHeader('Content-Type') || 'text/plain' } }));
    });
    xhr.addEventListener('error', () => reject(new TypeError('Failed to fetch')));
    xhr.addEventListener('abort', () => reject(new DOMException('The operation was aborted.', 'AbortError')));
    if (init.signal) {
      if (init.signal.aborted) { reject(new DOMException('The operation was aborted.', 'AbortError')); return; }
      init.signal.addEventListener('abort', () => xhr.abort(), { once: true });
    }
    xhr.send(init.body);
  });
}

/** Transport talking to the Yulius server that served this page. */
export function realTransport() {
  return {
    kind: 'real',
    fetch: (url, init) => window.fetch(url, init),
    upload: xhrUpload,
    openSocket(path) {
      const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
      return new WebSocket(`${proto}//${window.location.host}${path}`);
    },
  };
}

/** Percent-encode one path segment (ids, slugs, names). */
export const enc = (value) => encodeURIComponent(String(value));

/**
 * Create the API client. `onUnauthenticated` runs when a request returns 401
 * (session expired), unless the call passes {allow401: true}. A `signal` cancels the request
 * (a view that goes away aborts its reads); it then rejects with code `aborted`. A `Blob` or
 * `File` body is sent as it is (an upload, EDT-006); with `onProgress(sent, total)` the transport
 * reports its progress, and `timeoutMs` then counts from the last progress.
 */
export function createApi(transport, { onUnauthenticated } = {}) {
  let csrf = null;

  async function request(method, path, body, { allow401 = false, timeoutMs = REQUEST_TIMEOUT_MS, signal = null, onProgress = null, keepalive = false } = {}) {
    const headers = { Accept: 'application/json' };
    const init = { method, headers, credentials: 'same-origin', cache: 'no-store' };
    // Outlives the page (a save as it hides); browsers bound such bodies to 64 KiB in all.
    if (keepalive) init.keepalive = true;
    if (method !== 'GET') {
      if (csrf) headers['X-Yulius-CSRF'] = csrf;
      if (body instanceof Blob) {
        headers['Content-Type'] = 'application/octet-stream';
        init.body = body;
      } else if (body !== undefined) {
        headers['Content-Type'] = 'application/json';
        init.body = JSON.stringify(body);
      }
    }
    const controller = new AbortController();
    let timer = setTimeout(() => controller.abort(), timeoutMs);
    const progress = onProgress ? (sent, total) => {
      clearTimeout(timer);
      timer = setTimeout(() => controller.abort(), timeoutMs);
      onProgress(sent, total);
    } : null;
    const cancel = () => controller.abort();
    if (signal) {
      if (signal.aborted) cancel();
      else signal.addEventListener('abort', cancel, { once: true });
    }
    init.signal = controller.signal;
    let response;
    let text = '';
    try {
      response = await (progress && transport.upload ? transport.upload(path, init, progress) : transport.fetch(path, init));
      text = response.status === 204 ? '' : await response.text();
    } catch (error) {
      if (signal && signal.aborted) throw new ApiError(0, 'aborted', 'Cancelled.');
      if (error && error.name === 'AbortError') throw new ApiError(0, 'timeout', 'The server did not answer in time.');
      throw new ApiError(0, 'network', `Cannot reach the ${BRAND.name} server.`);
    } finally {
      clearTimeout(timer);
      if (signal) signal.removeEventListener('abort', cancel);
    }
    let data = null;
    if (text) {
      try { data = JSON.parse(text); } catch { data = text; }
    }
    if (!response.ok) {
      const payload = data && typeof data === 'object' && data.error ? data.error : null;
      const error = payload
        ? new ApiError(response.status, payload.code || STATUS_CODES[response.status] || 'error', payload.message || `Request failed (${response.status})`)
        : new ApiError(response.status, STATUS_CODES[response.status] || (response.status >= 500 ? 'internal' : 'error'), `Request failed (${response.status})`);
      // Extra fields beside `error` (for example `reauth` with a `reauth_required` refusal).
      if (data && typeof data === 'object') error.body = data;
      if (response.status === 401 && !allow401 && onUnauthenticated) onUnauthenticated(error);
      throw error;
    }
    return data;
  }

  return {
    transport,
    get: (path, options) => request('GET', path, undefined, options),
    post: (path, body, options) => request('POST', path, body, options),
    put: (path, body, options) => request('PUT', path, body, options),
    patch: (path, body, options) => request('PATCH', path, body, options),
    del: (path, options) => request('DELETE', path, undefined, options),
    setCsrf(token) { csrf = token || null; },
  };
}
