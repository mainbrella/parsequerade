import { ApiError, configurationError, notFound } from "./errors.js";
import { equal, sha256, unbase64url } from "./crypto.js";

export const identifier = /^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$/;
export const isObject = value => value !== null && typeof value === "object" && !Array.isArray(value);
const own = (object, key) => Object.hasOwn(object, key);

function configuredJson(value) {
  try {
    if (typeof value !== "string" || value.length > 5120) throw new Error();
    return JSON.parse(value);
  } catch { throw configurationError(); }
}

export async function authenticate(request, env, connectionId) {
  const bearer = /^Bearer ([A-Za-z0-9_-]{32,256})$/i.exec(request.headers.get("Authorization") || "");
  if (!bearer) throw new ApiError(401, "unauthorized", "Provide a valid facade API key.", { headers: { "WWW-Authenticate": "Bearer" } });
  const keys = configuredJson(env.FACADE_API_KEYS_JSON);
  if (!Array.isArray(keys) || !keys.length || keys.length > 100) throw configurationError();
  const hash = await sha256(bearer[1]);
  const ids = new Set(), hashes = new Set();
  let principal;
  for (const key of keys) {
    if (!isObject(key) || typeof key.id !== "string" || !identifier.test(key.id) ||
        typeof key.sha256 !== "string" || !/^[a-f0-9]{64}$/.test(key.sha256) ||
        !Array.isArray(key.grants) || ids.has(key.id) || hashes.has(key.sha256) ||
        (own(key, "revoked") && typeof key.revoked !== "boolean")) throw configurationError();
    ids.add(key.id); hashes.add(key.sha256);
    const connections = new Set();
    for (const grant of key.grants) {
      if (!isObject(grant) || typeof grant.connectionId !== "string" || !identifier.test(grant.connectionId) ||
          connections.has(grant.connectionId) || !Array.isArray(grant.capabilities) ||
          grant.capabilities.some(capability => !["read", "write", "reports"].includes(capability)) ||
          !(grant.appIds === "*" || (Array.isArray(grant.appIds) && grant.appIds.length <= 200 &&
            grant.appIds.every(id => typeof id === "string" && identifier.test(id))))) throw configurationError();
      connections.add(grant.connectionId);
    }
    if (equal(key.sha256, hash) && key.revoked !== true) principal = key;
  }
  if (!principal) throw new ApiError(401, "unauthorized", "Provide a valid facade API key.", { headers: { "WWW-Authenticate": "Bearer" } });
  const grant = principal.grants.find(g => g.connectionId === connectionId);
  if (!grant || !grant.capabilities.includes("read")) throw new ApiError(403, "forbidden", "This key cannot read this connection.");
  return { principalId: principal.id, grant, authorizationRevision: await sha256(JSON.stringify({ principal: principal.id, grant })) };
}

export function requireApp(auth, appId) {
  if (auth.grant.appIds !== "*" && !auth.grant.appIds.includes(appId)) throw notFound();
}

export function connectionConfig(env, connectionId) {
  const connections = configuredJson(env.APPLE_CONNECTIONS_JSON);
  if (!isObject(connections)) throw configurationError();
  if (!own(connections, connectionId)) throw notFound();
  const config = connections[connectionId];
  if (!isObject(config) || typeof config.issuerId !== "string" ||
      !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i.test(config.issuerId) ||
      typeof config.keyId !== "string" || !/^[A-Z0-9]{10}$/.test(config.keyId) ||
      typeof config.credentialRevision !== "string" || !identifier.test(config.credentialRevision) ||
      typeof config.privateKeySecret !== "string" || !/^APPLE_PRIVATE_KEY(?:_[A-Z0-9_]+)?$/.test(config.privateKeySecret) ||
      typeof env[config.privateKeySecret] !== "string") throw configurationError();
  let cursorSecret;
  try {
    if (typeof env.CURSOR_SIGNING_KEY !== "string" || env.CURSOR_SIGNING_KEY.length !== 43) throw new Error();
    cursorSecret = unbase64url(env.CURSOR_SIGNING_KEY);
    if (cursorSecret.length !== 32) throw new Error();
  } catch { throw configurationError(); }
  return { ...config, connectionId, privateKey: env[config.privateKeySecret], cursorSecret };
}
