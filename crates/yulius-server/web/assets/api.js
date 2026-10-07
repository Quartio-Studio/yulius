// HTTP API client and transport (docs/api.md).
// A transport is {kind, fetch(url, init) → Promise<Response>, openSocket(path) → WebSocket-like}.
// The real transport uses window.fetch / WebSocket; ?mock=1 swaps in assets/mock.js, so
// request building, error parsing and the WebSocket protocol code run unchanged in both.

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

const STATUS_CODES = { 400: 'invalid', 401: 'unauthenticated', 403: 'forbidden', 404: 'not_found', 409: 'conflict', 422: 'invalid', 429: 'capacity', 502: 'upstream' };

/** Transport talking to the Yulius server that served this page. */
export function realTransport() {
  return {
    kind: 'real',
    fetch: (url, init) => window.fetch(url, init),
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
 * (session expired), unless the call passes {allow401: true}.
 */
export function createApi(transport, { onUnauthenticated } = {}) {
  let csrf = null;

  async function request(method, path, body, { allow401 = false, timeoutMs = REQUEST_TIMEOUT_MS } = {}) {
    const headers = { Accept: 'application/json' };
    const init = { method, headers, credentials: 'same-origin', cache: 'no-store' };
    if (method !== 'GET') {
      if (csrf) headers['X-Yulius-CSRF'] = csrf;
      if (body !== undefined) {
        headers['Content-Type'] = 'application/json';
        init.body = JSON.stringify(body);
      }
    }
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    init.signal = controller.signal;
    let response;
    let text = '';
    try {
      response = await transport.fetch(path, init);
      text = response.status === 204 ? '' : await response.text();
    } catch (error) {
      if (error && error.name === 'AbortError') throw new ApiError(0, 'timeout', 'The server did not answer in time.');
      throw new ApiError(0, 'network', 'Cannot reach the Yulius server.');
    } finally {
      clearTimeout(timer);
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
