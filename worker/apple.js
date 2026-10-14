import { ApiError, configurationError, invalidUpstream } from "./errors.js";
import { base64url, bytes, sha256 } from "./crypto.js";
import { isObject } from "./auth.js";

export const APPLE_ORIGIN = "https://api.appstoreconnect.apple.com";
export const POLICY = Object.freeze({ requestMs: 25000, attemptMs: 8000, requests: 32, concurrentReads: 4,
  responseBytes: 4 * 1024 * 1024, totalBytes: 16 * 1024 * 1024, resolverPages: 20, cursorPages: 100 });

export function retryAfter(value, now) {
  if (!value) return null;
  const seconds = /^\d+$/.test(value) ? Number(value) : Math.ceil((Date.parse(value) - now) / 1000);
  return Number.isFinite(seconds) && seconds >= 0 ? Math.min(seconds, 86400) : null;
}

function upstreamError(status, payload, headers, now, route) {
  const classifications = {
    401: [502, "upstream_authentication_failed", "Apple rejected the configured credentials. Ask the operator to repair this connection."],
    403: [403, "upstream_forbidden", "The configured Apple key cannot read this resource."],
    404: [404, "resource_not_found", "The requested resource was not found."],
    409: [409, "upstream_conflict", "Apple reported a resource conflict."],
    400: [422, "validation_failed", "Apple rejected one or more request parameters."],
    422: [422, "validation_failed", "Apple rejected one or more request parameters."],
    429: [429, "rate_limited", "Apple is throttling this connection."],
  };
  const [mapped, code, message] = classifications[status] || [status >= 500 ? 503 : 502, "upstream_unavailable", "Apple could not complete this request."];
  const after = retryAfter(headers.get("Retry-After"), now);
  const names = new Map(route.parameters.filter(p => p.appleName).map(p => [p.appleName, p.name]));
  const details = Array.isArray(payload?.errors) ? payload.errors.slice(0, 32).map(error => {
    const detail = { code: typeof error?.code === "string" && /^[A-Za-z0-9_.-]{1,128}$/.test(error.code) ? error.code : "upstream_error", message };
    if (names.has(error?.source?.parameter)) detail.field = names.get(error.source.parameter);
    return detail;
  }) : [];
  // Apple error titles/details may contain private values; preserve codes, never raw messages.
  return new ApiError(mapped, code, message, { retryable: status === 429 || status >= 500,
    details, headers: after === null ? {} : { "Retry-After": String(after) } });
}

async function limitedJson(response, budget, signal) {
  const length = Number(response.headers.get("Content-Length"));
  if (Number.isFinite(length) && length > POLICY.responseBytes) throw invalidUpstream();
  const mime = response.headers.get("Content-Type")?.split(";")[0].trim().toLowerCase();
  if (response.ok && mime !== "application/json" && !/^application\/[a-z0-9.+-]+\+json$/.test(mime || "")) throw invalidUpstream();
  if (!response.body) return null;
  const reader = response.body.getReader();
  const abort = () => { void reader.cancel().catch(() => {}); };
  signal.addEventListener("abort", abort, { once: true });
  let lengthRead = 0;
  const chunks = [];
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      lengthRead += value.byteLength;
      budget.bytes += value.byteLength;
      if (lengthRead > POLICY.responseBytes || budget.bytes > POLICY.totalBytes) throw invalidUpstream();
      chunks.push(value);
    }
    const buffer = new Uint8Array(lengthRead);
    let offset = 0;
    for (const chunk of chunks) { buffer.set(chunk, offset); offset += chunk.byteLength; }
    try { return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(buffer)); }
    catch { if (response.ok) throw invalidUpstream(); return null; }
  } finally {
    signal.removeEventListener("abort", abort);
    if (lengthRead > POLICY.responseBytes || budget.bytes > POLICY.totalBytes) await reader.cancel();
    reader.releaseLock();
  }
}

export function createAppleTransport({ fetch: fetcher = globalThis.fetch, now = Date.now,
  sleep = ms => new Promise(resolve => setTimeout(resolve, ms)), random = Math.random } = {}) {
  const credentials = new Map();

  async function credential(config) {
    const fingerprint = await sha256(JSON.stringify([config.connectionId, config.issuerId, config.keyId, config.credentialRevision, config.privateKey]));
    if (!credentials.has(fingerprint)) {
      if (credentials.size >= 64) credentials.delete(credentials.keys().next().value);
      const state = { fingerprint };
      state.key = (async () => {
        try {
          const pem = /^-----BEGIN PRIVATE KEY-----\s+([A-Za-z0-9+/=\s]+)\s+-----END PRIVATE KEY-----\s*$/.exec(config.privateKey);
          if (!pem) throw new Error();
          const der = Uint8Array.from(atob(pem[1].replace(/\s/g, "")), c => c.charCodeAt(0));
          return await crypto.subtle.importKey("pkcs8", der, { name: "ECDSA", namedCurve: "P-256" }, false, ["sign"]);
        } catch { throw configurationError(); }
      })();
      // Await immediately below so failed imports cannot become unhandled rejections.
      credentials.set(fingerprint, state);
    }
    const state = credentials.get(fingerprint);
    try { await state.key; } catch (error) { credentials.delete(fingerprint); throw error; }
    return state;
  }

  async function token(config, state) {
    if (state.token && state.expires > now() + 60000) return state.token;
    if (!state.signing) {
      state.signing = (async () => {
        const seconds = Math.floor(now() / 1000);
        const header = base64url(bytes(JSON.stringify({ alg: "ES256", kid: config.keyId, typ: "JWT" })));
        const payload = base64url(bytes(JSON.stringify({ iss: config.issuerId, iat: seconds - 30, exp: seconds + 600, aud: "appstoreconnect-v1" })));
        const content = `${header}.${payload}`;
        const signature = new Uint8Array(await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, await state.key, bytes(content)));
        if (signature.length !== 64) throw configurationError();
        state.expires = (seconds + 600) * 1000;
        state.token = `${content}.${base64url(signature)}`;
        return state.token;
      })().finally(() => { state.signing = null; });
    }
    return state.signing;
  }

  return {
    credential,
    budget: () => ({ deadline: now() + POLICY.requestMs, requests: 0, bytes: 0 }),
    async get(config, state, url, budget, route) {
      if (url.origin !== APPLE_ORIGIN || url.username || url.password || url.hash || !/^\/v1\/[A-Za-z0-9/_-]+$/.test(url.pathname)) throw invalidUpstream();
      if ((state.active || 0) >= POLICY.concurrentReads) throw new ApiError(429, "rate_limited", "This connection has too many concurrent reads.", {
        retryable: true, headers: { "Retry-After": "1" },
      });
      state.active = (state.active || 0) + 1;
      try {
      let refreshed = false;
      for (let attempt = 0; attempt < 3; attempt++) {
        if (++budget.requests > POLICY.requests || budget.deadline <= now()) {
          throw new ApiError(504, "upstream_budget_exceeded", "The bounded Apple read could not finish.", { retryable: true });
        }
        const jwt = await token(config, state);
        const controller = new AbortController();
        const timer = setTimeout(() => controller.abort(), Math.min(POLICY.attemptMs, Math.max(1, budget.deadline - now())));
        let response, payload, error;
        const timeout = new Promise((_, reject) => controller.signal.addEventListener("abort", () => reject(
          new ApiError(504, "upstream_timeout", "Apple did not respond within the read deadline.", { retryable: true })), { once: true }));
        try {
          await Promise.race([(async () => {
            response = await fetcher(url.toString(), { method: "GET", redirect: "manual", signal: controller.signal,
              headers: { Authorization: `Bearer ${jwt}`, Accept: "application/json" } });
            if (controller.signal.aborted) throw new ApiError(504, "upstream_timeout", "Apple did not respond within the read deadline.", { retryable: true });
            if (response.status >= 300 && response.status < 400) throw invalidUpstream();
            payload = await limitedJson(response, budget, controller.signal);
          })(), timeout]);
          if (response.ok) {
            if (!isObject(payload) || !Object.hasOwn(payload, "data")) throw invalidUpstream();
            return payload;
          }
          error = upstreamError(response.status, payload, response.headers, now(), route);
        } catch (caught) {
          error = caught instanceof ApiError ? caught : new ApiError(503, "upstream_unavailable", "The Apple read could not complete.", { retryable: true });
        } finally {
          clearTimeout(timer);
          if (response?.body && !response.bodyUsed) { void response.body.cancel().catch(() => {}); }
        }
        if (response?.status === 401 && !refreshed && attempt < 2) {
          if (state.token === jwt) state.token = null;
          refreshed = true;
          continue;
        }
        const transient = error.retryable && [429, 503, 504].includes(error.status);
        if (!transient || attempt === 2) throw error;
        const delay = error.headers["Retry-After"] !== undefined ? Number(error.headers["Retry-After"]) * 1000 : Math.round(200 * 2 ** attempt + random() * 200);
        if (delay > 2000 || now() + delay >= budget.deadline) throw error;
        await sleep(delay);
      }
      } finally { state.active--; }
    },
  };
}
