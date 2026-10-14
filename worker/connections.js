import { isObject, identifier } from "./auth.js";
import { APPLE_ORIGIN } from "./apple.js";
import { base64url, bytes, equal, sha256, unbase64url } from "./crypto.js";
import { ApiError, invalidUpstream } from "./errors.js";

export const hostedConnectionId = /^conn_live_[a-f0-9]{32}$/;
const randomSecret = () => base64url(crypto.getRandomValues(new Uint8Array(32)));
const unauthorized = () => new ApiError(401, "unauthorized", "Provide a valid facade API key.", { headers: { "WWW-Authenticate": "Bearer" } });
const unavailable = () => new ApiError(503, "hosted_connections_not_configured", "The operator must configure encrypted hosted connection storage.");

async function encryptionKey(env) {
  try {
    if (typeof env.CONNECTION_ENCRYPTION_KEY !== "string" || env.CONNECTION_ENCRYPTION_KEY.length !== 43) throw new Error();
    const key = unbase64url(env.CONNECTION_ENCRYPTION_KEY);
    if (key.length !== 32) throw new Error();
    return await crypto.subtle.importKey("raw", key, "AES-GCM", false, ["encrypt", "decrypt"]);
  } catch { throw unavailable(); }
}

async function hostedStorage(env) {
  if (!env.HOSTED_CONNECTIONS?.getByName) throw unavailable();
  await encryptionKey(env);
  return env.HOSTED_CONNECTIONS;
}

async function inputCredentials(request) {
  if (request.headers.get("Content-Type")?.split(";")[0].trim().toLowerCase() !== "application/json") {
    throw new ApiError(415, "unsupported_media_type", "Send connection credentials as application/json.");
  }
  const tooLarge = () => new ApiError(413, "request_too_large", "Connection credentials must fit within 8 KiB.");
  if (Number(request.headers.get("Content-Length")) > 8192) throw tooLarge();
  if (!request.body) throw new ApiError(400, "invalid_request", "Provide issuerId, keyId, and privateKey.");
  const reader = request.body.getReader();
  const chunks = [];
  let size = 0, timer;
  try {
    const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(new ApiError(408, "request_timeout", "The connection request body did not arrive in time.")), 5000); });
    while (true) {
      const { value, done } = await Promise.race([reader.read(), timeout]);
      if (done) break;
      size += value.byteLength;
      if (size > 8192) throw tooLarge();
      chunks.push(value);
    }
    const buffer = new Uint8Array(size);
    let offset = 0;
    for (const chunk of chunks) { buffer.set(chunk, offset); offset += chunk.byteLength; }
    let input;
    try { input = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(buffer)); }
    catch { throw new ApiError(400, "invalid_request", "Provide a valid JSON connection request."); }
    if (!isObject(input) || Object.keys(input).some(key => !["issuerId", "keyId", "privateKey"].includes(key)) ||
        typeof input.issuerId !== "string" || !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i.test(input.issuerId) ||
        typeof input.keyId !== "string" || !/^[A-Z0-9]{10}$/.test(input.keyId) ||
        typeof input.privateKey !== "string" || input.privateKey.length > 4096) {
      throw new ApiError(400, "invalid_request", "Provide an issuer UUID, a 10-character Apple key ID, and a PKCS#8 privateKey PEM.");
    }
    return input;
  } finally {
    clearTimeout(timer);
    void reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

async function storageRequest(env, connectionId, method, { body, authorization } = {}) {
  const namespace = await hostedStorage(env);
  let response;
  try {
    response = await namespace.getByName(connectionId).fetch(new Request(`https://connection.internal/${connectionId}`, {
      method, headers: { "Content-Type": "application/json", ...(authorization ? { Authorization: authorization } : {}) },
      ...(body ? { body: JSON.stringify(body) } : {}),
    }));
  } catch { throw new ApiError(503, "connection_storage_unavailable", "Connection storage is temporarily unavailable.", { retryable: true }); }
  if (response.status === 204) return null;
  const result = await response.json();
  if (!response.ok) throw new ApiError(response.status, result.error.code, result.error.message, result.error);
  return result;
}

export async function createHostedConnection(request, env, transport) {
  await hostedStorage(env);
  if (!env.CONNECTION_SETUP_LIMITER?.limit) throw unavailable();
  const { success } = await env.CONNECTION_SETUP_LIMITER.limit({ key: `parsequerade:connection-setup:${request.headers.get("CF-Connecting-IP") || "local"}` });
  if (!success) throw new ApiError(429, "rate_limited", "Too many connection setup attempts. Try again in a minute.", { retryable: true, headers: { "Retry-After": "60" } });
  if (new URL(request.url).search) throw new ApiError(400, "invalid_request", "Connection setup does not accept query parameters.");
  const input = await inputCredentials(request);
  const connectionId = `conn_live_${crypto.randomUUID().replaceAll("-", "")}`;
  const config = { ...input, connectionId, credentialRevision: "1", cursorSecret: randomSecret() };
  let credential;
  try { credential = await transport.credential(config); }
  catch (error) {
    if (error instanceof ApiError && error.code === "service_not_configured") {
      throw new ApiError(400, "invalid_credentials", "Provide a valid P-256 PKCS#8 Apple private key.");
    }
    throw error;
  }
  // Probe only the read capability being issued; no administrator-only /users access or Apple writes.
  let probe;
  try { probe = await transport.get(config, credential, new URL("/v1/apps?limit=1", APPLE_ORIGIN), transport.budget(), { parameters: [] }); }
  catch (error) {
    if (error instanceof ApiError && error.code === "upstream_authentication_failed") {
      throw new ApiError(422, "invalid_credentials", "Apple rejected this issuer ID, key ID, or private key.");
    }
    throw error;
  }
  if (!Array.isArray(probe.data) || probe.data.some(app => !isObject(app) || app.type !== "apps" || typeof app.id !== "string" || !identifier.test(app.id))) throw invalidUpstream();
  const apiKey = `pq_${randomSecret()}`;
  await storageRequest(env, connectionId, "PUT", { body: { config, apiKeyHash: await sha256(apiKey) } });
  return { connectionId, baseUrl: `${new URL(request.url).origin}/v1/connections/${connectionId}`, apiKey };
}

export async function readHostedConnection(request, env, connectionId) {
  const result = await storageRequest(env, connectionId, "GET", { authorization: request.headers.get("Authorization") });
  return { ...result, config: { ...result.config, cursorSecret: unbase64url(result.config.cursorSecret) } };
}

export async function deleteHostedConnection(request, env, connectionId) {
  await storageRequest(env, connectionId, "DELETE", { authorization: request.headers.get("Authorization") });
}

// This class is reachable only through the Worker binding, never through a public route.
export class HostedConnection {
  constructor(ctx, env) { this.storage = ctx.storage; this.env = env; }

  async fetch(request) {
    try {
      const connectionId = new URL(request.url).pathname.slice(1);
      if (!hostedConnectionId.test(connectionId)) throw new ApiError(400, "invalid_path", "Invalid hosted connection identifier.");
      if (request.method === "PUT") {
        const { config, apiKeyHash } = await request.json();
        const iv = crypto.getRandomValues(new Uint8Array(12));
        const encrypted = await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: bytes(connectionId) }, await encryptionKey(this.env), bytes(JSON.stringify(config)));
        await this.storage.transaction(async txn => {
          if (await txn.get("connection")) throw new ApiError(409, "connection_exists", "This connection already exists.");
          await txn.put("connection", { connectionId, apiKeyHash, iv: base64url(iv), encrypted: base64url(new Uint8Array(encrypted)) });
        });
        return new Response(null, { status: 204 });
      }
      if (!["GET", "DELETE"].includes(request.method)) throw new ApiError(405, "method_not_allowed", "Unsupported connection storage method.");
      const bearer = /^Bearer ([A-Za-z0-9_-]{32,256})$/i.exec(request.headers.get("Authorization") || "");
      if (!bearer) throw unauthorized();
      const hash = await sha256(bearer[1]);
      const record = await this.storage.transaction(async txn => {
        const stored = await txn.get("connection");
        if (!stored || stored.connectionId !== connectionId || !equal(stored.apiKeyHash, hash)) throw unauthorized();
        if (request.method === "DELETE") await txn.delete("connection");
        return stored;
      });
      if (request.method === "DELETE") return new Response(null, { status: 204 });
      let config;
      try {
        const decrypted = await crypto.subtle.decrypt({ name: "AES-GCM", iv: unbase64url(record.iv), additionalData: bytes(connectionId) }, await encryptionKey(this.env), unbase64url(record.encrypted));
        config = JSON.parse(new TextDecoder().decode(decrypted));
      } catch { throw unavailable(); }
      const grant = { connectionId, capabilities: ["read"], appIds: "*" };
      const auth = { principalId: connectionId, grant, authorizationRevision: await sha256(JSON.stringify({ principal: connectionId, keyHash: hash, grant })) };
      return Response.json({ config, auth });
    } catch (caught) {
      const error = caught instanceof ApiError ? caught : new ApiError(503, "connection_storage_unavailable", "Connection storage is temporarily unavailable.", { retryable: true });
      return Response.json({ error: { code: error.code, message: error.message, retryable: error.retryable, details: error.details, headers: error.headers } }, { status: error.status });
    }
  }
}
