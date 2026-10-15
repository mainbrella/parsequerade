import { generateKeyPairSync, createHash, verify, randomBytes } from "node:crypto";
import assert from "node:assert/strict";

export const pair = generateKeyPairSync("ec", { namedCurve: "prime256v1" });
export const facadeKey = `pq_${randomBytes(32).toString("base64url")}`;
export const connectionId = "conn_example";
export const base = `https://facade.example/v1/connections/${connectionId}`;
export const pem = pair.privateKey.export({ type: "pkcs8", format: "pem" });
export const sha = value => createHash("sha256").update(value).digest("hex");

export function makeEnv(appIds = "*") {
  return {
    APPLE_CONNECTIONS_JSON: JSON.stringify({ [connectionId]: { issuerId: "00000000-1111-2222-3333-444444444444", keyId: "ABCDEF1234", credentialRevision: "1", privateKeySecret: "APPLE_PRIVATE_KEY" } }),
    APPLE_PRIVATE_KEY: pem, CURSOR_SIGNING_KEY: randomBytes(32).toString("base64url"),
    FACADE_API_KEYS_JSON: JSON.stringify([{ id: "test_client", sha256: sha(facadeKey), grants: [{ connectionId, capabilities: ["read"], appIds }] }]),
    ASSETS: { fetch: async () => new Response("static", { headers: { "Content-Type": "text/html" } }) },
  };
}

export function validateJwt(authorization) {
  assert.match(authorization, /^Bearer /);
  const token = authorization.slice(7);
  assert.notEqual(token, facadeKey);
  const [header, payload, signature] = token.split(".");
  assert.equal(JSON.parse(Buffer.from(header, "base64url")).alg, "ES256");
  const claims = JSON.parse(Buffer.from(payload, "base64url"));
  assert.equal(claims.aud, "appstoreconnect-v1");
  assert.equal(claims.iss, "00000000-1111-2222-3333-444444444444");
  assert.ok(claims.exp - claims.iat <= 1200);
  assert.ok(verify("sha256", Buffer.from(`${header}.${payload}`), { key: pair.publicKey, dsaEncoding: "ieee-p1363" }, Buffer.from(signature, "base64url")));
  return token;
}

export function json(data, next = null) { return { data, links: { next } }; }
export function resource(type, id, attributes = {}, relationships) { return { type, id, attributes, ...(relationships ? { relationships } : {}) }; }
export const app = (id = "123", attributes = {}) => resource("apps", id, { name: "Example Notes", bundleId: "com.example.notes", ...attributes });
export const version = (id = "version_1", attributes = {}) => resource("appStoreVersions", id, { platform: "IOS", appVersionState: "PREPARE_FOR_SUBMISSION", ...attributes });
export const appInfo = (id = "info_1") => resource("appInfos", id, { state: "PREPARE_FOR_SUBMISSION" });
export const localization = (locale = "en-US", id = "locale_1", attributes = {}) => resource("appStoreVersionLocalizations", id, { locale, description: "Example description", ...attributes });

export function jsonResponse(payload, status = 200, headers = {}) {
  return new Response(JSON.stringify(payload), { status, headers: { "Content-Type": "application/json", ...headers } });
}

export function request(path = "/apps", options = {}) {
  return new Request(base + path, { ...options, headers: { Authorization: `Bearer ${facadeKey}`, ...options.headers } });
}

export function readFixture(route) {
  const path = route.route.replace("{appId}", "123").replace("{appInfoId}", "info_1").replace("{versionId}", "version_1").replace("{locale}", "en-US");
  let item;
  switch (route.type) {
    case "apps": item = app(); break;
    case "appInfos": item = appInfo(); break;
    case "appStoreVersions": item = version(); break;
    case "appInfoLocalizations": item = resource(route.type, "locale_1", { locale: "en-US", name: "Example Notes", privacyPolicyUrl: null }); break;
    case "appStoreVersionLocalizations": item = localization(); break;
    case "appStoreReviewDetails": item = resource(route.type, "review_1", { contactEmail: "review@example.com", notes: "private", demoAccountPassword: "secret" }); break;
    case "ageRatingDeclarations": item = resource(route.type, "rating_1", { gambling: false, ageRatingOverrideV2: "FUTURE_READ_VALUE" }); break;
  }
  const responses = [];
  if (path.includes("info_1")) responses.push(json([appInfo()]));
  if (path.includes("version_1")) responses.push(json([version()]));
  if (path.endsWith("/en-US")) responses.push(json([item]));
  responses.push(json(route.collection ? [item] : item));
  return { path, item, responses };
}
