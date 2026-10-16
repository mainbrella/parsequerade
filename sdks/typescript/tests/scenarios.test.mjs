import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { Client, ApiError } from "../dist/index.js";
import { pem, validateJwt } from "../../../tests/helpers.mjs";

const scenarios = JSON.parse(await readFile(new URL("../../../tests/sdk/scenarios.json", import.meta.url), "utf8"));
const camel = value => value.replace(/_([a-z])/g, (_, letter) => letter.toUpperCase());

function subset(actual, expected) {
  if (Array.isArray(expected)) {
    assert.ok(Array.isArray(actual));
    assert.equal(actual.length, expected.length);
    expected.forEach((item, index) => subset(actual[index], item));
  } else if (expected !== null && typeof expected === "object") {
    for (const [key, value] of Object.entries(expected)) {
      assert.ok(Object.hasOwn(actual, key), `Missing ${key}`);
      subset(actual[key], value);
    }
  } else assert.equal(actual, expected);
}

test("TypeScript executes all shared Rust and Python operation scenarios", async t => {
  assert.equal(scenarios.length, 18);
  for (const scenario of scenarios) await t.test(scenario.name, async () => {
    let calls = 0;
    const client = await Client.create({
      issuerId: "00000000-1111-2222-3333-444444444444", keyId: "ABCDEF1234", privateKey: pem,
      fetch: async (url, init) => {
        const step = scenario.steps[calls++];
        assert.ok(step, `Unexpected request ${init.method} ${url}`);
        const parsed = new URL(url);
        assert.equal(parsed.origin, "https://api.appstoreconnect.apple.com");
        assert.equal(parsed.pathname, step.path);
        assert.equal(init.method, step.method);
        assert.deepEqual(Object.fromEntries(parsed.searchParams), step.query ?? {});
        assert.deepEqual(init.body ? JSON.parse(init.body) : null, step.body ?? null);
        validateJwt(init.headers.Authorization);
        return Response.json(step.response, { status: step.status ?? 200 });
      },
    });
    try {
      const method = camel(scenario.pythonMethod ?? scenario.operation);
      const args = scenario.pythonArgs ?? scenario.args;
      if (scenario.error) {
        await assert.rejects(client[method](...args), error => {
          assert.ok(error instanceof ApiError);
          subset(error, scenario.error);
          return true;
        });
      } else subset(await client[method](...args), scenario.expect);
      assert.equal(calls, scenario.steps.length);
    } finally { client.close(); }
  });
});

test("closing clients cancels pending reads without blocking the Node event loop", async () => {
  let contacted;
  const started = new Promise(resolve => { contacted = resolve; });
  const client = await Client.create({
    issuerId: "00000000-1111-2222-3333-444444444444", keyId: "ABCDEF1234", privateKey: pem,
    fetch: async () => { contacted(); return new Promise(() => {}); },
  });
  const pending = client.apps();
  await started;
  await new Promise(resolve => setImmediate(resolve));
  client.close();
  client.close();
  await assert.rejects(pending, error => error instanceof ApiError && error.code === "operation_cancelled");
  await assert.rejects(client.apps(), error => error instanceof ApiError && error.code === "client_closed");
});

test("closing immediately never dispatches an already queued HTTP event", async () => {
  let calls = 0;
  const client = await Client.create({
    issuerId: "00000000-1111-2222-3333-444444444444", keyId: "ABCDEF1234", privateKey: pem,
    fetch: async () => { calls++; throw new Error("Unexpected dispatch after close"); },
  });
  const pending = client.apps();
  client.close();
  await assert.rejects(pending, error => error instanceof ApiError && error.code === "operation_cancelled");
  assert.equal(calls, 0);
});

test("an expired write event is rejected before custom fetch dispatch", async () => {
  const addon = createRequire(import.meta.url)("../dist/parsequerade.node");
  const poll = addon.pollJson;
  addon.pollJson = async (...args) => {
    const event = JSON.parse(await poll(...args));
    if (event?.kind === "http" && event.method !== "GET") event.deadlineMs = Date.now() - 1;
    return JSON.stringify(event);
  };
  let calls = 0;
  const client = await Client.create({
    issuerId: "00000000-1111-2222-3333-444444444444", keyId: "ABCDEF1234", privateKey: pem,
    fetch: async (_url, init) => {
      calls++;
      assert.equal(init.method, "GET", "Expired write must not be dispatched");
      return Response.json({ data: { type: "apps", id: "app1", attributes: {} } });
    },
  });
  try {
    await assert.rejects(client.createVersion("app1", "IOS", "2.0"), error =>
      error instanceof ApiError && error.code === "write_outcome_unknown");
    assert.equal(calls, 1, "Only the ownership read was dispatched");
  } finally { addon.pollJson = poll; client.close(); }
});
