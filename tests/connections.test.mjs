import { test } from "node:test";
import assert from "node:assert/strict";
import { randomBytes, generateKeyPairSync } from "node:crypto";
import { createWorker } from "../worker/index.js";
import { HostedConnection } from "../worker/connections.js";
import { pem } from "./helpers.mjs";

const issuerId = "00000000-1111-2222-3333-444444444444";
const keyId = "ABCDEF1234";
const applePayload = { data: [], links: { next: null } };

function makeHostedEnv({ encryptionKey = randomBytes(32).toString("base64url"), limiterSuccess = true } = {}) {
  const records = new Map();
  const calls = { limiter: [], apple: [], storage: [] };
  const env = {
    CONNECTION_ENCRYPTION_KEY: encryptionKey,
    CONNECTION_SETUP_LIMITER: {
      async limit(input) {
        calls.limiter.push(input);
        return { success: limiterSuccess };
      },
    },
    HOSTED_CONNECTIONS: {
      getByName(name) {
        return {
          async fetch(request) {
            calls.storage.push({ name, method: request.method });
            const data = records.get(name) ?? new Map();
            records.set(name, data);
            const storage = {
              async transaction(callback) {
                return callback({
                  get: async key => data.get(key),
                  put: async (key, value) => { data.set(key, structuredClone(value)); },
                  delete: async key => { data.delete(key); },
                });
              },
            };
            // A fresh class instance on each operation models a DO eviction/restart.
            return new HostedConnection({ storage }, env).fetch(request);
          },
        };
      },
    },
  };
  return { env, records, calls };
}

function workerFixture(options = {}) {
  const hosted = makeHostedEnv(options);
  const logs = [];
  const worker = createWorker({
    log: event => logs.push(event),
    fetch: async (url, init) => {
      hosted.calls.apple.push({ url: new URL(url), init });
      return new Response(JSON.stringify(applePayload), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    },
  });
  return { ...hosted, logs, worker };
}

function createRequest(body = { issuerId, keyId, privateKey: pem }, options = {}) {
  return new Request("https://facade.example/v1/connections", {
    method: "POST",
    headers: { "Content-Type": "application/json", "CF-Connecting-IP": "203.0.113.9", ...options.headers },
    body: typeof body === "string" ? body : JSON.stringify(body),
    ...options.request,
  });
}

async function createConnection(fixture, request = createRequest()) {
  const response = await fixture.worker.fetch(request, fixture.env);
  return { response, body: await response.json() };
}

test("hosted connection can be created, read after an instance restart, and revoked", async () => {
  const fixture = workerFixture();
  const { response, body } = await createConnection(fixture);
  assert.equal(response.status, 201);
  assert.match(body.connectionId, /^conn_live_[a-f0-9]{32}$/);
  assert.equal(body.baseUrl, `https://facade.example/v1/connections/${body.connectionId}`);
  assert.match(body.apiKey, /^pq_[A-Za-z0-9_-]{43}$/);
  assert.equal(response.headers.get("Location"), body.baseUrl);
  assert.equal(fixture.calls.apple.length, 1);
  assert.equal(fixture.calls.apple[0].url.pathname, "/v1/apps");
  assert.equal(fixture.calls.apple[0].url.search, "?limit=1");
  assert.equal(fixture.calls.apple[0].init.method, "GET");

  const stored = fixture.records.get(body.connectionId).get("connection");
  assert.deepEqual(Object.keys(stored).sort(), ["apiKeyHash", "connectionId", "encrypted", "iv"]);
  const persisted = JSON.stringify(stored);
  assert.equal(persisted.includes(pem), false);
  assert.equal(persisted.includes(body.apiKey), false);
  assert.equal(persisted.includes("privateKey"), false);
  assert.equal(persisted.includes("issuerId"), false);

  // A distinct worker uses a new Durable Object class instance backed by the same storage.
  const secondWorker = createWorker({
    log: event => fixture.logs.push(event),
    fetch: async () => new Response(JSON.stringify(applePayload), {
      headers: { "Content-Type": "application/json" },
    }),
  });
  const authorization = `Bearer ${body.apiKey}`;
  const read = await secondWorker.fetch(new Request(`${body.baseUrl}/apps`, { headers: { Authorization: authorization } }), fixture.env);
  assert.equal(read.status, 200);
  assert.deepEqual(await read.json(), { items: [], nextCursor: null });

  const deletion = await secondWorker.fetch(new Request(body.baseUrl, { method: "DELETE", headers: { Authorization: authorization } }), fixture.env);
  assert.equal(deletion.status, 204);
  assert.equal(fixture.records.get(body.connectionId).has("connection"), false);
  const revoked = await secondWorker.fetch(new Request(`${body.baseUrl}/apps`, { headers: { Authorization: authorization } }), fixture.env);
  assert.equal(revoked.status, 401);
});

test("hosted facade keys cannot read or revoke another connection", async () => {
  const fixture = workerFixture();
  const first = await createConnection(fixture);
  const second = await createConnection(fixture);
  assert.equal(first.response.status, 201);
  assert.equal(second.response.status, 201);

  const request = (id, method = "GET") => new Request(`https://facade.example/v1/connections/${id}${method === "GET" ? "/apps" : ""}`, {
    method,
    headers: { Authorization: `Bearer ${first.body.apiKey}` },
  });
  const read = await fixture.worker.fetch(request(second.body.connectionId), fixture.env);
  assert.equal(read.status, 401);
  const deletion = await fixture.worker.fetch(request(second.body.connectionId, "DELETE"), fixture.env);
  assert.equal(deletion.status, 401);
  assert.ok(fixture.records.get(second.body.connectionId).has("connection"));
});

test("setup rejects malformed, oversized, unsupported, and unknown input before Apple", async t => {
  const cases = [
    ["wrong media type", { body: JSON.stringify({ issuerId, keyId, privateKey: pem }), headers: { "Content-Type": "text/plain" } }, 415],
    ["malformed JSON", { body: "{" }, 400],
    ["unknown property", { body: { issuerId, keyId, privateKey: pem, access: "admin" } }, 400],
    ["invalid issuer", { body: { issuerId: "issuer", keyId, privateKey: pem } }, 400],
    ["invalid key ID", { body: { issuerId, keyId: "short", privateKey: pem } }, 400],
    ["invalid private key type", { body: { issuerId, keyId, privateKey: 3 } }, 400],
    ["invalid private key PEM", { body: { issuerId, keyId, privateKey: "not a private key" } }, 400],
    ["wrong elliptic curve", { body: { issuerId, keyId, privateKey: generateKeyPairSync("ec", { namedCurve: "secp384r1" }).privateKey.export({ type: "pkcs8", format: "pem" }) } }, 400],
    ["declared oversized body", { body: { issuerId, keyId, privateKey: pem }, headers: { "Content-Length": "8193" } }, 413],
  ];
  for (const [name, config, status] of cases) await t.test(name, async () => {
    const fixture = workerFixture();
    const request = createRequest(config.body, { headers: config.headers });
    const response = await fixture.worker.fetch(request, fixture.env);
    assert.equal(response.status, status);
    assert.equal(fixture.calls.apple.length, 0);
    assert.equal(fixture.records.size, 0);
  });

  const fixture = workerFixture();
  const streamingBody = new ReadableStream({
    start(controller) { controller.enqueue(new TextEncoder().encode(JSON.stringify({ issuerId, keyId, privateKey: pem }) + " ".repeat(8193))); controller.close(); },
  });
  const streamed = await fixture.worker.fetch(new Request("https://facade.example/v1/connections", {
    method: "POST", headers: { "Content-Type": "application/json" }, body: streamingBody, duplex: "half",
  }), fixture.env);
  assert.equal(streamed.status, 413);
  assert.equal(fixture.calls.apple.length, 0);
  assert.equal(fixture.records.size, 0);
});

test("only a successful Apple capability probe creates persistent credentials", async t => {
  for (const status of [401, 403]) await t.test(`Apple ${status}`, async () => {
    const fixture = workerFixture();
    fixture.worker = createWorker({
      log: event => fixture.logs.push(event),
      fetch: async (url, init) => {
        fixture.calls.apple.push({ url: new URL(url), init });
        return new Response(JSON.stringify({ errors: [{ detail: "secret upstream detail" }] }), {
        status, headers: { "Content-Type": "application/json" },
        });
      },
    });
    const result = await createConnection(fixture);
    assert.notEqual(result.response.status, 201);
    assert.equal(fixture.records.size, 0);
    assert.equal(fixture.calls.apple.length, status === 401 ? 2 : 1);
    assert.equal(JSON.stringify(result.body).includes("secret upstream detail"), false);
  });

  const invalidShape = workerFixture();
  invalidShape.worker = createWorker({
    log: () => {},
    fetch: async (url, init) => {
      invalidShape.calls.apple.push({ url: new URL(url), init });
      return new Response(JSON.stringify({ data: [{ type: "apps", id: "bad/id" }] }), {
        headers: { "Content-Type": "application/json" },
      });
    },
  });
  const bad = await createConnection(invalidShape);
  assert.equal(bad.response.status, 502);
  assert.equal(invalidShape.records.size, 0);
});

test("missing hosted storage or encryption configuration fails closed; limiter denial skips Apple", async t => {
  for (const [name, change] of [
    ["namespace", env => { delete env.HOSTED_CONNECTIONS; }],
    ["encryption key", env => { env.CONNECTION_ENCRYPTION_KEY = "invalid"; }],
    ["setup limiter", env => { delete env.CONNECTION_SETUP_LIMITER; }],
  ]) await t.test(name, async () => {
    const fixture = workerFixture();
    change(fixture.env);
    const response = await fixture.worker.fetch(createRequest(), fixture.env);
    assert.equal(response.status, 503);
    assert.equal(fixture.calls.apple.length, 0);
  });

  const limited = workerFixture({ limiterSuccess: false });
  const response = await limited.worker.fetch(createRequest(), limited.env);
  assert.equal(response.status, 429);
  assert.equal(response.headers.get("Retry-After"), "60");
  assert.equal(limited.calls.apple.length, 0);
  assert.equal(limited.records.size, 0);
});

test("tampered ciphertext and changed encryption key fail closed", async t => {
  const original = workerFixture();
  const created = await createConnection(original);
  assert.equal(created.response.status, 201);
  const record = original.records.get(created.body.connectionId).get("connection");
  const request = () => new Request(`${created.body.baseUrl}/apps`, {
    headers: { Authorization: `Bearer ${created.body.apiKey}` },
  });

  await t.test("modified ciphertext", async () => {
    const bytes = Buffer.from(record.encrypted, "base64url");
    bytes[bytes.length - 1] ^= 1;
    original.records.get(created.body.connectionId).set("connection", { ...record, encrypted: bytes.toString("base64url") });
    const response = await original.worker.fetch(request(), original.env);
    assert.equal(response.status, 503);
    assert.equal((await response.json()).error.code, "hosted_connections_not_configured");
    assert.equal(original.calls.apple.length, 1);
  });

  await t.test("rotated encryption key", async () => {
    const fixture = workerFixture();
    const prior = await createConnection(fixture);
    assert.equal(prior.response.status, 201);
    fixture.env.CONNECTION_ENCRYPTION_KEY = randomBytes(32).toString("base64url");
    const response = await fixture.worker.fetch(new Request(`${prior.body.baseUrl}/apps`, {
      headers: { Authorization: `Bearer ${prior.body.apiKey}` },
    }), fixture.env);
    assert.equal(response.status, 503);
    assert.equal(fixture.calls.apple.length, 1);
  });
});

test("setup audit events never contain Apple private keys or issued facade bearer keys", async () => {
  const fixture = workerFixture();
  const created = await createConnection(fixture);
  assert.equal(created.response.status, 201);
  const serialized = JSON.stringify(fixture.logs);
  assert.equal(serialized.includes(pem), false);
  assert.equal(serialized.includes(created.body.apiKey), false);
  assert.equal(serialized.includes(`Bearer ${created.body.apiKey}`), false);
});
