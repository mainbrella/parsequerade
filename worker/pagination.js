import { APPLE_ORIGIN, POLICY } from "./apple.js";
import { ApiError, invalidUpstream } from "./errors.js";
import { base64url, bytes, digest, unbase64url } from "./crypto.js";
import { canonicalQuery } from "./query.js";

export const CURSOR_TTL_MS = 15 * 60 * 1000;

export async function pageHash(url) { return base64url((await digest(url.toString())).slice(0, 16)); }

export function nextPage(link, initial) {
  if (link === undefined || link === null) return null;
  if (typeof link !== "string" || link.length > 8192) throw invalidUpstream();
  let url;
  try { url = new URL(link, initial); } catch { throw invalidUpstream(); }
  if (url.origin !== APPLE_ORIGIN || url.pathname !== initial.pathname || url.username || url.password || url.hash) throw invalidUpstream();
  const keys = new Set();
  for (const [key, value] of url.searchParams) {
    if (keys.has(key) || (key !== "cursor" && (!initial.searchParams.has(key) || value !== initial.searchParams.get(key)))) throw invalidUpstream();
    keys.add(key);
  }
  const cursor = url.searchParams.get("cursor");
  if (!cursor || cursor.length > 2048) throw invalidUpstream();
  // Preserve filters, scopes, projections and page sizes even if Apple's link omits them.
  const result = new URL(initial);
  result.searchParams.set("cursor", cursor);
  result.searchParams.sort();
  return result;
}

export function traversalError() {
  return new ApiError(502, "upstream_incomplete_result", "Apple pagination exceeded the bounded traversal or repeated a page.");
}

export async function cursorCodec(secret, now = Date.now) {
  const key = await crypto.subtle.importKey("raw", secret, { name: "HMAC", hash: "SHA-256" }, false, ["sign", "verify"]);
  return {
    async encode(binding, next, seen, expires = now() + CURSOR_TTL_MS) {
      const payload = base64url(bytes(JSON.stringify({ v: 1, binding, next, seen, expires })));
      const signature = base64url(new Uint8Array(await crypto.subtle.sign("HMAC", key, bytes(payload))));
      const token = `${payload}.${signature}`;
      if (token.length > 8192) throw invalidUpstream();
      return token;
    },
    async decode(token, binding) {
      try {
        if (token.length > 8192) throw new Error();
        const parts = token.split(".");
        if (parts.length !== 2 || parts[1].length !== 43 || !await crypto.subtle.verify("HMAC", key, unbase64url(parts[1]), bytes(parts[0]))) throw new Error();
        const payload = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(unbase64url(parts[0])));
        if (payload.v !== 1 || payload.binding !== binding || !Number.isSafeInteger(payload.expires) ||
            payload.expires <= now() || payload.expires > now() + CURSOR_TTL_MS ||
            typeof payload.next !== "string" || !payload.next || payload.next.length > 2048 ||
            !Array.isArray(payload.seen) || payload.seen.length < 1 || payload.seen.length > POLICY.cursorPages ||
            payload.seen.some(hash => typeof hash !== "string" || !/^[A-Za-z0-9_-]{22}$/.test(hash)) ||
            new Set(payload.seen).size !== payload.seen.length) throw new Error();
        return payload;
      } catch { throw new ApiError(400, "invalid_cursor", "The cursor is invalid, expired, or belongs to another request."); }
    },
  };
}

export function appleUrl(path, query = new URLSearchParams()) {
  const url = new URL(path, APPLE_ORIGIN);
  url.search = canonicalQuery(query);
  return url;
}
