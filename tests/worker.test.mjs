import { test } from "node:test";
import assert from "node:assert/strict";
import { createWorker } from "../worker/index.js";
import { routes } from "../worker/resources.js";
import { CURSOR_TTL_MS } from "../worker/pagination.js";
import { base, connectionId, facadeKey, makeEnv, request, app, appInfo, version, localization, resource, json, jsonResponse, validateJwt } from "./helpers.mjs";

function fixture(responses, env = makeEnv(), extra = {}) {
  const calls = [], logs = [];
  const worker = createWorker({ sleep: async () => {}, random: () => 0, log: event => logs.push(event), ...extra,
    fetch: async (url, init) => {
      const call = { url: new URL(url), init }; calls.push(call);
      assert.equal(call.url.origin, "https://api.appstoreconnect.apple.com");
      assert.equal(init.method, "GET"); assert.equal(init.redirect, "manual"); validateJwt(init.headers.Authorization);
      const result = responses[calls.length - 1];
      if (typeof result === "function") return result(call);
      if (result instanceof Response) return result;
      assert.ok(result, `Unexpected upstream call ${call.url.pathname}`);
      return jsonResponse(result);
    },
  });
  return { env, calls, logs, worker, get: (path, options) => worker.fetch(request(path, options), env) };
}

test("flat app list maps reviewed query parameters and signs a valid Apple JWT", async () => {
  const f = fixture([json([app("123", { isOrEverWasMadeForKids: false, subscriptionStatusUrl: null, futureField: "hidden" })])]);
  const response = await f.get("/apps?bundleId=com.example.notes&sort=-name&limit=2");
  assert.equal(response.status, 200);
  const body = await response.json();
  assert.equal(body.items[0].name, "Example Notes"); assert.equal(body.items[0].isOrEverWasMadeForKids, false);
  assert.equal(body.items[0].subscriptionStatusUrl, null); assert.equal(body.items[0].futureField, undefined);
  assert.equal(body.nextCursor, null); assert.equal(body.items[0].attributes, undefined);
  assert.equal(f.calls[0].url.searchParams.get("filter[bundleId]"), "com.example.notes");
  assert.equal(f.calls[0].url.searchParams.get("sort"), "-name");
  assert.match(response.headers.get("Cache-Control"), /no-store/);
  assert.equal(response.headers.get("X-Request-Id"), f.logs[0].requestId);
  assert.equal(JSON.stringify(f.logs).includes(facadeKey), false);
  assert.equal(f.logs[0].route, "/apps");
});

test("all twelve routes return the documented flat resource and parent context", async t => {
  for (const route of routes) await t.test(route.handler, async () => {
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
    const f = fixture(responses);
    const result = await f.get(path);
    assert.equal(result.status, 200);
    const body = await result.json();
    const value = route.collection ? body.items[0] : body;
    assert.equal(value.id, item.id);
    if (!route.handler.startsWith("app") || !["app", "apps"].includes(route.handler)) assert.equal(value.appId, "123");
    assert.equal(value.demoAccountPassword, undefined); assert.equal(value.notes, undefined);
    if (route.handler === "ageRating") { assert.equal(value.gambling, false); assert.equal(value.ageRatingOverrideV2, "FUTURE_READ_VALUE"); }
    assert.equal(f.calls.length, responses.length);
  });
});

test("sparse fields never change ownership or locale selection", async () => {
  const f = fixture([json([version()]), json([localization()]), json(localization("en-US", "locale_1", { keywords: "notes" }))]);
  const response = await f.get("/apps/123/versions/version_1/localizations/en-US?fields=description");
  assert.deepEqual(await response.json(), { id: "locale_1", appId: "123", versionId: "version_1", description: "Example description" });
  assert.equal(f.calls[0].url.pathname, "/v1/apps/123/appStoreVersions");
  assert.equal(f.calls.at(-1).url.searchParams.get("fields[appStoreVersionLocalizations]"), "description,locale");
});

test("missing/invalid/revoked keys and missing capability never contact Apple", async t => {
  const cases = [
    ["missing", {}, { Authorization: "" }, 401], ["invalid", {}, { Authorization: "Bearer invalid" }, 401],
    ["revoked", { revoked: true }, {}, 401], ["connection", { grants: [{ connectionId: "other", capabilities: ["read"], appIds: "*" }] }, {}, 403],
    ["capability", { grants: [{ connectionId, capabilities: ["write"], appIds: "*" }] }, {}, 403],
  ];
  for (const [name, change, headers, status] of cases) await t.test(name, async () => {
    const env = makeEnv(); const keys = JSON.parse(env.FACADE_API_KEYS_JSON); Object.assign(keys[0], change); env.FACADE_API_KEYS_JSON = JSON.stringify(keys);
    const f = fixture([], env); assert.equal((await f.get("/apps", { headers })).status, status); assert.equal(f.calls.length, 0);
  });
});

test("restricted app grants constrain Apple queries and deny nested access before transport", async () => {
  const f = fixture([json([app()])], makeEnv(["123", "456"]));
  assert.equal((await f.get("/apps?id=123,789")).status, 200);
  assert.equal(f.calls[0].url.searchParams.get("filter[id]"), "123");
  assert.deepEqual(await (await f.get("/apps?id=789")).json(), { items: [], nextCursor: null });
  assert.equal((await f.get("/apps/789/versions/version_1")).status, 404);
  assert.equal(f.calls.length, 1);
  const rogue = fixture([json([app("789")])], makeEnv(["123"]));
  assert.equal((await rogue.get("/apps")).status, 502);
});

test("nested IDs are proven through scoped parent membership, including later pages", async () => {
  const f = fixture([json([appInfo("other")], "https://api.appstoreconnect.apple.com/v1/apps/123/appInfos?cursor=next"), json([appInfo()]), json(appInfo())]);
  assert.equal((await f.get("/apps/123/app-infos/info_1")).status, 200);
  assert.equal(f.calls[1].url.searchParams.get("limit"), "200");
  const denied = fixture([json([version("other")])]);
  assert.equal((await denied.get("/apps/123/versions/version_1/localizations")).status, 404);
  assert.equal(denied.calls.length, 1);
  const mismatch = fixture([json([resource("appInfos", "info_1", {}, { app: { data: { type: "apps", id: "other" } } })])]);
  assert.equal((await mismatch.get("/apps/123/app-infos/info_1")).status, 502);
});

test("exact locale resolution traverses every page and rejects duplicate or missing matches", async () => {
  const missing = fixture([json([version()]), json([localization("fr-FR")])]);
  const response = await missing.get("/apps/123/versions/version_1/localizations/en-US");
  assert.equal(response.status, 404); assert.equal((await response.json()).error.code, "localization_not_found");
  const duplicate = fixture([json([version()]), json([localization()], "/v1/appStoreVersions/version_1/appStoreVersionLocalizations?cursor=next"), json([localization("en-US", "locale_2")])]);
  assert.equal((await duplicate.get("/apps/123/versions/version_1/localizations/en-US")).status, 409);
  const changed = fixture([json([version()]), json([localization()]), json(localization("fr-FR"))]);
  assert.equal((await changed.get("/apps/123/versions/version_1/localizations/en-US")).status, 502);
});

test("signed pagination preserves queries and rejects tampering, changed filters, callers and revisions", async () => {
  let time = Date.now();
  const f = fixture([json([app()], "/v1/apps?cursor=next"), json([app("456")])], makeEnv(), { now: () => time });
  const cursor = (await (await f.get("/apps?limit=1&sort=name")).json()).nextCursor;
  assert.ok(cursor);
  const encoded = encodeURIComponent(cursor);
  assert.equal((await f.get(`/apps?limit=1&sort=name&cursor=${encodeURIComponent(cursor.slice(0, -3) + "xyz")}`)).status, 400);
  assert.equal((await f.get(`/apps?limit=2&sort=name&cursor=${encoded}`)).status, 400);
  const keys = JSON.parse(f.env.FACADE_API_KEYS_JSON); keys[0].id = "changed_client"; f.env.FACADE_API_KEYS_JSON = JSON.stringify(keys);
  assert.equal((await f.get(`/apps?limit=1&sort=name&cursor=${encoded}`)).status, 400);
  keys[0].id = "test_client"; f.env.FACADE_API_KEYS_JSON = JSON.stringify(keys);
  const connections = JSON.parse(f.env.APPLE_CONNECTIONS_JSON); connections[connectionId].credentialRevision = "2"; f.env.APPLE_CONNECTIONS_JSON = JSON.stringify(connections);
  assert.equal((await f.get(`/apps?limit=1&sort=name&cursor=${encoded}`)).status, 400);
  connections[connectionId].credentialRevision = "1"; f.env.APPLE_CONNECTIONS_JSON = JSON.stringify(connections);
  assert.equal((await f.get(`/apps?sort=name&limit=1&cursor=${encoded}`)).status, 200);
  assert.equal(f.calls.at(-1).url.searchParams.get("sort"), "name"); assert.equal(f.calls.at(-1).url.searchParams.get("limit"), "1");
  time += CURSOR_TTL_MS;
  assert.equal((await f.get(`/apps?limit=1&sort=name&cursor=${encoded}`)).status, 400);
  assert.equal(f.calls.length, 2);
});

test("cursors cannot move across connections, routes or changed app grants", async () => {
  const env = makeEnv(["123"]);
  const connections = JSON.parse(env.APPLE_CONNECTIONS_JSON); connections.other = { ...connections[connectionId] }; env.APPLE_CONNECTIONS_JSON = JSON.stringify(connections);
  const keys = JSON.parse(env.FACADE_API_KEYS_JSON); keys[0].grants.push({ connectionId: "other", appIds: ["123"], capabilities: ["read"] }); env.FACADE_API_KEYS_JSON = JSON.stringify(keys);
  const f = fixture([json([app()], "/v1/apps?cursor=next")], env);
  const cursor = encodeURIComponent((await (await f.get("/apps")).json()).nextCursor);
  assert.equal((await f.worker.fetch(new Request(`https://facade.example/v1/connections/other/apps?cursor=${cursor}`, { headers: { Authorization: `Bearer ${facadeKey}` } }), env)).status, 400);
  assert.equal((await f.get(`/apps/123/versions?cursor=${cursor}`)).status, 400);
  keys[0].grants[0].appIds = ["123", "456"]; env.FACADE_API_KEYS_JSON = JSON.stringify(keys);
  assert.equal((await f.get(`/apps?cursor=${cursor}`)).status, 400);
  assert.equal(f.calls.length, 1);
});

test("foreign, looping, duplicate-query and scope-changing next links fail", async t => {
  for (const link of ["https://evil.example/v1/apps?cursor=x", "/v1/users?cursor=x", "/v1/apps?cursor=x&cursor=y", "/v1/apps?cursor=x&filter[id]=789", "/v1/apps?limit=99&cursor=x", "/v1/apps?cursor=x#fragment"]) {
    await t.test(link, async () => { const f = fixture([json([app()], link)], makeEnv(["123"])); assert.equal((await f.get("/apps")).status, 502); assert.equal(f.calls.length, 1); });
  }
  const loop = fixture([json([app()], "/v1/apps?cursor=x"), json([app("456")], "/v1/apps?cursor=x")]);
  const cursor = encodeURIComponent((await (await loop.get("/apps")).json()).nextCursor);
  assert.equal((await loop.get(`/apps?cursor=${cursor}`)).status, 502);
});

test("unknown queries, relationship fields, duplicate queries and mutations are rejected before Apple", async t => {
  for (const path of ["/apps?include=versions", "/apps?filter[id]=123", "/apps?limit=0", "/apps?limit=201", "/apps?limit=1&limit=2", "/apps?fields=appStoreVersions", "/apps?sort=-createdDate", "/apps/123/versions?platform=ANDROID", "/apps/123/versions/a%2Fb"]) await t.test(path, async () => {
    const f = fixture([]); assert.equal((await f.get(path)).status, 400); assert.equal(f.calls.length, 0);
  });
  const f = fixture([]);
  for (const method of ["POST", "PATCH", "DELETE", "HEAD", "OPTIONS"]) {
    const response = await f.get("/apps/123", { method }); assert.equal(response.status, 405); assert.equal(response.headers.get("Allow"), "GET");
  }
  assert.equal((await f.get("/sales-reports")).status, 404); assert.equal(f.calls.length, 0);
});

test("Apple permission, auth, validation, throttling and service failures stay distinct", async t => {
  for (const [status, expected, code] of [[401, 502, "upstream_authentication_failed"], [403, 403, "upstream_forbidden"], [404, 404, "resource_not_found"], [422, 422, "validation_failed"], [429, 429, "rate_limited"], [500, 503, "upstream_unavailable"]]) await t.test(String(status), async () => {
    const payload = { errors: [{ code: "FIRST", detail: "private-secret", source: { parameter: "filter[bundleId]" } }, { code: "SECOND", title: "private-secret" }] };
    const response = () => jsonResponse(payload, status, status === 429 ? { "Retry-After": "30" } : {});
    const f = fixture([response, response, response]); const result = await f.get("/apps?bundleId=com.example.notes");
    assert.equal(result.status, expected); const body = await result.json(); assert.equal(body.error.code, code); assert.equal(body.error.details.length, 2);
    assert.equal(body.error.details[0].field, "bundleId"); assert.equal(JSON.stringify(body).includes("private-secret"), false);
    assert.equal(f.calls.length, status === 401 ? 2 : status === 500 ? 3 : 1);
    if (status === 429) assert.equal(result.headers.get("Retry-After"), "30");
  });
});

test("JWTs are reused, refreshed once on rejection, and isolated on credential changes", async () => {
  const f = fixture([json(app()), json(app()), jsonResponse({}, 401), json(app()), json(app())]);
  assert.equal((await f.get("/apps/123")).status, 200); assert.equal((await f.get("/apps/123")).status, 200);
  assert.equal(f.calls[0].init.headers.Authorization, f.calls[1].init.headers.Authorization);
  assert.equal((await f.get("/apps/123")).status, 200);
  assert.notEqual(f.calls[2].init.headers.Authorization, f.calls[3].init.headers.Authorization);
  const config = JSON.parse(f.env.APPLE_CONNECTIONS_JSON); config[connectionId].credentialRevision = "2"; f.env.APPLE_CONNECTIONS_JSON = JSON.stringify(config);
  assert.equal((await f.get("/apps/123")).status, 200);
  assert.notEqual(f.calls[3].init.headers.Authorization, f.calls[4].init.headers.Authorization);
});

test("invalid payloads, redirects and excessive bodies never become successful empty data", async t => {
  for (const response of [new Response("<html>error</html>", { headers: { "Content-Type": "text/html" } }), jsonResponse({}), jsonResponse(json({ type: "apps", id: "123", attributes: [] })), jsonResponse(json([app(), app()])), jsonResponse(json([app()], null), 200, { "Content-Length": "5000000" }), new Response(null, { status: 302, headers: { Location: "https://evil.example" } }), jsonResponse(json(app("wrong")))]) {
    await t.test("reject malformed response", async () => { const f = fixture([response]); const collection = response.headers.get("Content-Length") || response.status === 302 || response === undefined; const result = await f.get(collection ? "/apps" : "/apps/123"); assert.equal(result.status, 502); });
  }
});

test("resolver page budgets fail explicitly instead of selecting from incomplete data", async () => {
  const responses = [json([version()]), ...Array.from({ length: 20 }, (_, i) => json([], `/v1/appStoreVersions/version_1/appStoreVersionLocalizations?cursor=page${i}`))];
  const f = fixture(responses); const result = await f.get("/apps/123/versions/version_1/localizations/en-US");
  assert.equal(result.status, 502); assert.equal((await result.json()).error.code, "upstream_incomplete_result"); assert.equal(f.calls.length, 21);
});

test("timeout after dispatch returns a retryable read error within the total budget", async t => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let time = Date.now(), entered;
  const dispatched = new Promise(resolve => { entered = resolve; });
  const f = fixture([async () => { time += 25000; entered(); return new Promise(() => {}); }], makeEnv(), { now: () => time });
  const pending = f.get("/apps"); await dispatched; t.mock.timers.tick(8000);
  const result = await pending; assert.equal(result.status, 504); assert.equal((await result.json()).error.retryable, true); assert.equal(f.calls.length, 1);
});

test("the per-isolate connection concurrency limit bounds simultaneous Apple reads", async () => {
  let finish;
  const gate = new Promise(resolve => { finish = resolve; });
  const worker = createWorker({ log: () => {}, fetch: async () => { await gate; return jsonResponse(json([app()])); } });
  const env = makeEnv();
  const pending = Array.from({ length: 5 }, () => worker.fetch(request(), env));
  const rejected = await Promise.race(pending); assert.equal(rejected.status, 429); finish();
  const results = await Promise.all(pending); assert.equal(results.filter(r => r.status === 200).length, 4);
});

test("unconfigured API fails closed while static assets stay public", async () => {
  const f = fixture([], { ASSETS: makeEnv().ASSETS });
  assert.equal((await f.get("/apps")).status, 503);
  assert.equal((await f.worker.fetch(new Request("https://facade.example/docs/"), f.env)).status, 200);
  for (const path of ["/v1", "/v1/unknown", "/v1/connections/conn_example/unknown"]) {
    const response = await f.worker.fetch(new Request("https://facade.example" + path), f.env);
    assert.match(response.headers.get("Content-Type"), /application\/json/);
  }
  const bad = fixture([], { ...makeEnv(), APPLE_PRIVATE_KEY: "invalid" });
  assert.equal((await bad.get("/apps")).status, 503); assert.equal(bad.calls.length, 0);
});
