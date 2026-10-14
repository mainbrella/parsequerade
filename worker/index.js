import { authenticate, connectionConfig, identifier, requireApp } from "./auth.js";
import { createAppleTransport } from "./apple.js";
import { ApiError, jsonResponse, notFound } from "./errors.js";
import { cursorCodec } from "./pagination.js";
import { queryFor } from "./query.js";
import { matchRoute, readResource } from "./resources.js";

// Dependency injection is confined to construction, never exposed through HTTP or environment config.
export function createWorker(dependencies = {}) {
  const transport = createAppleTransport(dependencies);
  const now = dependencies.now || Date.now;
  const log = dependencies.log || (event => console.log(JSON.stringify(event)));
  return {
    async fetch(request, env) {
      const url = new URL(request.url);
      if (!/^\/v1(?:\/|$)/.test(url.pathname)) return env.ASSETS.fetch(request);
      const requestId = `req_${crypto.randomUUID()}`;
      const started = now();
      const audit = { requestId, method: request.method };
      let response;
      try {
        if (url.pathname.length + url.search.length > 12288) throw new ApiError(400, "invalid_request", "The request URL exceeds the size limit.");
        const base = /^\/v1\/connections\/([^/]+)(\/.*)?$/.exec(url.pathname);
        if (!base) throw notFound("route_not_found");
        let connectionId;
        try { connectionId = decodeURIComponent(base[1]); } catch { throw new ApiError(400, "invalid_path", "The connection path is invalid."); }
        if (!identifier.test(connectionId)) throw new ApiError(400, "invalid_path", "The connection path is invalid.");
        const auth = await authenticate(request, env, connectionId);
        audit.connectionId = connectionId; audit.principalId = auth.principalId;
        const resourcePath = (base[2] || "").replace(/\/$/, "");
        const match = matchRoute(resourcePath);
        if (!match) throw notFound("route_not_found");
        audit.route = match.route.route;
        if (request.method !== "GET") throw new ApiError(405, "method_not_allowed", "This resource currently supports GET only.", { headers: { Allow: "GET" } });
        if (match.params.appId) requireApp(auth, match.params.appId);
        const query = queryFor(match.route, url.searchParams);
        const config = connectionConfig(env, connectionId);
        const credential = await transport.credential(config);
        const codec = await cursorCodec(config.cursorSecret, now);
        const body = await readResource({ ...match, auth, config, credential, codec, query, resourcePath, budget: transport.budget() }, transport);
        response = jsonResponse(body, 200, requestId);
      } catch (caught) {
        const error = caught instanceof ApiError ? caught : new ApiError(500, "internal_error", "The service could not complete this request.");
        audit.errorCode = error.code;
        response = jsonResponse({ error: { code: error.code, message: error.message, requestId, retryable: error.retryable, details: error.details } }, error.status, requestId, error.headers);
      }
      // Never log bearer tokens, queries/cursors, Apple payloads, PEMs or exception messages.
      try { log({ ...audit, status: response.status, durationMs: now() - started }); } catch { /* Logging cannot change a read result. */ }
      return response;
    },
  };
}

export default createWorker();
