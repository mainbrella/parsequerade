import { test } from "node:test";
import assert from "node:assert/strict";
import { createWorker } from "../worker/index.js";
import { makeEnv, request, json, resource, appInfo, jsonResponse, validateJwt, connectionId } from "./helpers.mjs";

const path = "/apps/123/app-infos/info_1/age-rating";
function fixture({ capabilities = ["read", "write"], appIds = "*", infos = [appInfo()], rating = resource("ageRatingDeclarations", "target_rating", { gambling: false }), result, writeStatus = 200, failure } = {}) {
  const env = makeEnv(appIds), calls = [];
  const keys = JSON.parse(env.FACADE_API_KEYS_JSON); keys[0].grants[0].capabilities = capabilities; env.FACADE_API_KEYS_JSON = JSON.stringify(keys);
  const worker = createWorker({ log: () => {}, sleep: async () => { throw new Error("Must not retry a write"); }, fetch: async (raw, init) => {
    const url = new URL(raw); calls.push({ url, init }); validateJwt(init.headers.Authorization);
    if (init.method === "PATCH") {
      if (failure) throw new Error("network failed");
      return jsonResponse(result || (writeStatus === 200 ? json(resource("ageRatingDeclarations", "target_rating", JSON.parse(init.body).data.attributes)) : { errors: [{ code: "APPLE_ERROR", detail: "secret" }] }), writeStatus);
    }
    assert.equal(init.method, "GET");
    if (url.pathname === "/v1/apps/123/appInfos") return jsonResponse(json(infos));
    assert.equal(url.pathname, "/v1/appInfos/info_1/ageRatingDeclaration");
    return jsonResponse(json(rating));
  } });
  const post = (body = { gambling: true }, options = {}) => worker.fetch(request(path, { method: "POST", body: typeof body === "string" ? body : JSON.stringify(body), headers: { "Content-Type": "application/json", ...options.headers }, ...options }), env);
  return { env, calls, post, worker };
}

test("POST changes only the resolved target age declaration and returns a flat resource", async () => {
  const f = fixture();
  const attributes = { gambling: false, alcoholTobaccoOrDrugUseOrReferences: "INFREQUENT", kidsAgeBand: null, ageRatingOverrideV2: "NONE", gracRatingClassificationNumber: null, developerAgeRatingInfoUrl: "https://example.com/ratings" };
  const response = await f.post(attributes);
  assert.equal(response.status, 200);
  assert.match(response.headers.get("Cache-Control"), /no-store/);
  assert.deepEqual(await response.json(), { id: "target_rating", appId: "123", appInfoId: "info_1", ...attributes });
  assert.deepEqual(f.calls.map(call => call.init.method), ["GET", "GET", "PATCH"]);
  assert.equal(f.calls[2].url.pathname, "/v1/ageRatingDeclarations/target_rating");
  assert.deepEqual(JSON.parse(f.calls[2].init.body), { data: { type: "ageRatingDeclarations", id: "target_rating", attributes } });
});

test("facade authentication, write capability, and app grant fail before Apple", async t => {
  for (const [name, options, expected] of [["read only", { capabilities: ["read"] }, 403], ["write without read", { capabilities: ["write"] }, 403], ["another app", { appIds: ["456"] }, 404]]) {
    await t.test(name, async () => {
      const f = fixture(options); const response = await f.post();
      assert.equal(response.status, expected); assert.equal(f.calls.length, 0);
      if (name === "read only") assert.equal((await response.json()).error.code, "write_forbidden");
    });
  }
  const f = fixture();
  const response = await f.worker.fetch(new Request(`https://facade.example/v1/connections/${connectionId}${path}`, { method: "POST", body: "{}" }), f.env);
  assert.equal(response.status, 401); assert.equal(f.calls.length, 0);
});

test("POST proves app-info ownership and resolves a declaration before any write", async t => {
  for (const [name, options, expected] of [["missing target info", { infos: [appInfo("another_info")] }, 404], ["mismatched app relationship", { infos: [resource("appInfos", "info_1", {}, { app: { data: { type: "apps", id: "456" } } })] }, 502], ["missing rating", { rating: null }, 404], ["invalid rating id", { rating: resource("ageRatingDeclarations", "bad/id") }, 502]]) await t.test(name, async () => {
    const f = fixture(options); const response = await f.post();
    assert.equal(response.status, expected); assert.equal(f.calls.some(call => call.init.method === "PATCH"), false);
  });
});

test("invalid attributes, enums, null answers, types and identifiers never contact Apple", async t => {
  for (const body of [{}, [], null, { id: "source_rating" }, { appId: "456" }, { __proto__: null, relationships: {} }, { ageRatingOverride: "NONE" }, { gambling: "false" }, { gambling: null }, { ageRatingOverrideV2: "FUTURE_VALUE" }, { kidsAgeBand: "ADULT" }, { developerAgeRatingInfoUrl: "javascript:alert(1)" }, { developerAgeRatingInfoUrl: "https://user:password@example.com" }, { gracRatingClassificationNumber: "x".repeat(2049) }, "{"]) await t.test(JSON.stringify(body).slice(0, 80), async () => {
    const f = fixture(); const response = await f.post(body); assert.equal(response.status, 400); assert.equal(f.calls.length, 0);
  });
  const unsupported = fixture(); assert.equal((await unsupported.post({}, { headers: { "Content-Type": "text/plain" } })).status, 415); assert.equal(unsupported.calls.length, 0);
  const oversized = fixture(); assert.equal((await oversized.post(" ".repeat(8193))).status, 413); assert.equal(oversized.calls.length, 0);
  const query = fixture(); assert.equal((await query.worker.fetch(request(path + "?fields=gambling", { method: "POST", headers: { "Content-Type": "application/json" }, body: '{"gambling":true}' }), query.env)).status, 400); assert.equal(query.calls.length, 0);
});

test("Apple write failures and network errors are never retried or marked retryable", async t => {
  for (const status of [401, 403, 409, 422, 429, 500, 503]) await t.test(String(status), async () => {
    const f = fixture({ writeStatus: status }); const response = await f.post(); const body = await response.json();
    assert.notEqual(response.status, 200); assert.equal(body.error.retryable, false);
    assert.equal(f.calls.filter(call => call.init.method === "PATCH").length, 1); assert.equal(f.calls.length, 3);
    assert.equal(JSON.stringify(body).includes("secret"), false);
  });
  const f = fixture({ failure: true }); const response = await f.post();
  assert.equal(response.status, 503); const body = await response.json(); assert.equal(body.error.retryable, false); assert.equal(body.error.code, "write_outcome_unknown"); assert.equal(f.calls.length, 3);
});

test("POST rejects an unexpected declaration in Apple's update response", async () => {
  const f = fixture({ result: json(resource("ageRatingDeclarations", "source_rating", { gambling: true })) });
  const response = await f.post(); assert.equal(response.status, 502); assert.equal((await response.json()).error.code, "write_outcome_unknown"); assert.equal(f.calls.length, 3);
});
