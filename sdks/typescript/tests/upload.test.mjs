import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { Client, ApiError } from "../dist/index.js";
import { pem, validateJwt } from "../../../tests/helpers.mjs";

const common = JSON.parse(await readFile(new URL("../../../tests/sdk/uploads.json", import.meta.url), "utf8"));
const PNG = Buffer.from(common.png, "base64");
const { request, binding, receipt } = common;
const complete = common.cases.find(value => value.name === "complete_chunked_upload");
const resume = common.cases.find(value => value.name === "resume_placement");
const clone = value => structuredClone(value);
const origin = new URL(complete.steps[9].path, complete.steps[8].payload.data.attributes.uploadOperations[0].url).origin;
const scope = () => clone(complete.steps.slice(0, 3));
const placement = () => clone(complete.steps[18].payload.data[0]);
const placements = (...items) => {
  const steps = clone(complete.steps.slice(3, 6));
  steps[2].payload.data = items;
  return steps;
};
const catalog = complete.steps[7].payload;
const preflight = () => clone(complete.steps.slice(0, 8));
const uploadOperation = (path = "/upload", offset = 0, length = PNG.length) => ({
  ...clone(complete.steps[8].payload.data.attributes.uploadOperations[0]), url: origin + path, offset, length,
});
const reservation = (operations = [uploadOperation()], error) => {
  const step = clone(complete.steps[8]);
  step.payload.data.attributes.uploadOperations = operations;
  step.error = error;
  return step;
};
const transfer = (path = "/upload", body = PNG) => ({ ...clone(complete.steps[9]), path, body, bodyBase64: undefined });
const commit = error => ({ ...clone(complete.steps[11]), error });
const createPlacement = () => clone(complete.steps[15]);
const imageRead = (state = "UPLOAD_COMPLETE") => {
  const step = clone(resume.steps.at(-1));
  step.payload.data.attributes.state = state;
  return step;
};
const checkpoint = phase => {
  const result = { ...clone(resume.checkpoint), phase };
  if (!phase.startsWith("placement_")) delete result.placementGroup;
  return result;
};

async function fixture(steps) {
  const calls = [];
  let failure;
  const client = await Client.create({ issuerId: "00000000-1111-2222-3333-444444444444", keyId: "ABCDEF1234", privateKey: pem,
    fetch: async (url, init) => {
      try {
        const expected = steps[calls.length];
        calls.push({ url: new URL(url), init });
        assert.ok(expected, `Unexpected ${init.method} ${url}`);
        const parsed = new URL(url);
        assert.equal(parsed.pathname, expected.path);
        assert.equal(init.method, expected.method);
        assert.deepEqual(Object.fromEntries(parsed.searchParams), expected.query);
        assert.equal(init.redirect, "manual");
        if (parsed.origin === "https://api.appstoreconnect.apple.com") validateJwt(init.headers.Authorization);
        else {
          assert.equal(parsed.origin, origin);
          assert.equal(new Headers(init.headers).has("authorization"), false);
        }
        for (const [name, value] of Object.entries(expected.headers ?? {})) assert.equal(new Headers(init.headers).get(name), value);
        const body = expected.bodyBase64 ? Buffer.from(expected.bodyBase64, "base64") : expected.body ?? undefined;
        assert.deepEqual(Buffer.isBuffer(body) ? init.body : init.body ? JSON.parse(init.body) : undefined, body);
        if (expected.error) throw expected.error;
        return Response.json(expected.payload, { status: expected.status ?? 200 });
      } catch (error) {
        if (error instanceof assert.AssertionError) failure = error;
        throw error;
      }
    },
  });
  return { client, calls, verify() { if (failure) throw failure; assert.equal(calls.length, steps.length); client.close(); } };
}
const errorCode = code => error => { assert.ok(error instanceof ApiError); assert.equal(error.code, code); assert.equal(error.retryable, false); return true; };
const noop = async () => {};

test("TypeScript executes the common upload and recovery scenarios", async t => {
  assert.equal(common.cases.length, 6);
  for (const scenario of common.cases) await t.test(scenario.name, async () => {
    const f = await fixture(clone(scenario.steps));
    const state = clone(scenario.checkpoint ?? {});
    const original = new Error("checkpoint persistence unavailable");
    const persist = scenario.persistFailure ? async () => { throw original; } : noop;
    const pending = f.client.uploadListingImage(request, PNG, state, persist);
    if (scenario.persistFailure) await assert.rejects(pending, error => error === original);
    else if (scenario.error) await assert.rejects(pending, errorCode(scenario.error));
    else assert.deepEqual(await pending, scenario.expect);
    f.verify();
  });
});

test("listing image reference data is read without writes", async () => {
  const f = await fixture([clone(complete.steps[7])]);
  assert.deepEqual(await f.client.listingImageReferenceData(), catalog);
  f.verify();
});

test("upload dispatches exact chunks and independent durable checkpoints before a verified receipt", async () => {
  const half = Math.floor(PNG.length / 2);
  const f = await fixture([...preflight(), reservation([
    uploadOperation("/upload/second", half, PNG.length - half), uploadOperation("/upload/first", 0, half),
  ]), transfer("/upload/second", PNG.subarray(half)), transfer("/upload/first", PNG.subarray(0, half)), commit(),
  ...placements(), createPlacement(), ...placements(placement())]);
  const state = {}, saves = [];
  assert.deepEqual(await f.client.uploadListingImage(request, PNG, state, async snapshot => { saves.push(snapshot); await Promise.resolve(); }), receipt);
  assert.equal(state.phase, "placement_created");
  assert.equal(state.placementId, "placement1");
  assert.deepEqual(saves.map(value => value.phase), ["reservation_started", "reserved", "upload_started", "commit_started", "committed", "placement_started", "placement_created"]);
  assert.equal(new Set(saves).size, saves.length);
  assert.ok(saves.every(value => value !== state));
  assert.equal(saves[0].imageId, undefined);
  assert.equal(saves[1].imageId, "image1");
  assert.ok(!JSON.stringify(state).includes("signed-fixture"));
  assert.ok(!JSON.stringify(state).includes(origin));
  f.verify();
});

test("uncertain reservations update the caller checkpoint and never replay", async () => {
  const f = await fixture([...preflight(), reservation(undefined, new Error("private transport failure"))]);
  const state = {};
  await assert.rejects(f.client.uploadListingImage(request, PNG, state, noop), errorCode("write_outcome_unknown"));
  assert.equal(state.phase, "reservation_started");
  assert.equal(state.imageId, undefined);
  await assert.rejects(f.client.uploadListingImage(request, PNG, state, noop), errorCode("write_outcome_unknown"));
  assert.equal(f.calls.filter(call => call.init.method === "POST").length, 1);
  f.verify();
});

test("uncertain placement resumes using reads and verifies the image", async () => {
  const f = await fixture([...scope(), ...placements(placement()), imageRead()]);
  const state = checkpoint("placement_started");
  assert.deepEqual(await f.client.uploadListingImage(request, PNG, state, noop), receipt);
  assert.equal(state.phase, "placement_created");
  assert.equal(state.placementId, "placement1");
  assert.ok(f.calls.every(call => call.init.method === "GET"));
  f.verify();
});

test("uncertain placement without matching readback never posts", async () => {
  const f = await fixture([...scope(), ...placements()]);
  await assert.rejects(f.client.uploadListingImage(request, PNG, checkpoint("placement_started"), noop), errorCode("write_outcome_unknown"));
  assert.ok(f.calls.every(call => call.init.method === "GET"));
  f.verify();
});

test("uncertain commit preserves its checkpoint and resume never repeats the PATCH", async () => {
  const f = await fixture([...preflight(), reservation(), transfer(), commit(new Error("lost response"))]);
  const state = {};
  await assert.rejects(f.client.uploadListingImage(request, PNG, state, noop), errorCode("write_outcome_unknown"));
  assert.equal(state.phase, "commit_started");
  const resumed = await fixture([...preflight(), imageRead("AWAITING_UPLOAD")]);
  await assert.rejects(resumed.client.uploadListingImage(request, PNG, state, noop), errorCode("write_outcome_unknown"));
  assert.ok(resumed.calls.every(call => call.init.method === "GET"));
  f.verify(); resumed.verify();
});

test("persistence failures preserve the original rejection and prevent writes", async () => {
  const f = await fixture(preflight());
  const state = {}, original = new Error("disk unavailable");
  await assert.rejects(f.client.uploadListingImage(request, PNG, state, async snapshot => {
    assert.equal(snapshot.phase, "reservation_started");
    throw original;
  }), error => error === original);
  assert.equal(state.phase, "reservation_started");
  assert.ok(f.calls.every(call => call.init.method === "GET"));
  f.verify();
});

test("cancellation retains the checkpoint lease until the original persistence callback settles", async () => {
  const f = await fixture(preflight());
  const state = {};
  let contacted, finishPersistence;
  const persisting = new Promise(resolve => { contacted = resolve; });
  const blocked = new Promise(resolve => { finishPersistence = resolve; });
  const pending = f.client.uploadListingImage(request, PNG, state, async snapshot => {
    contacted();
    await blocked;
    snapshot.phase = "late callback mutation";
  });
  await persisting;
  f.client.close();
  await assert.rejects(pending, errorCode("write_outcome_unknown"));
  assert.equal(state.phase, "reservation_started");
  assert.ok(f.calls.every(call => call.init.method === "GET"));
  const resumed = await fixture([]);
  await assert.rejects(resumed.client.uploadListingImage(request, PNG, state, noop), errorCode("checkpoint_in_use"));
  finishPersistence();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(state.phase, "reservation_started", "The original callback still holds an independent snapshot");
  await assert.rejects(resumed.client.uploadListingImage(request, PNG, state, noop), errorCode("write_outcome_unknown"));
  resumed.verify();
  f.verify();
});

test("persist callback mutation and caller mutation cannot forge the native checkpoint", async () => {
  const f = await fixture([...preflight(), reservation(undefined, new Error("lost response"))]);
  const state = {};
  await assert.rejects(f.client.uploadListingImage(request, PNG, state, async snapshot => {
    for (const key of Object.keys(snapshot)) delete snapshot[key];
    snapshot.phase = "forged";
    state.phase = "forged";
  }), errorCode("write_outcome_unknown"));
  assert.equal(state.phase, "reservation_started");
  assert.equal(state.binding, binding);
  f.verify();
});

test("the same checkpoint cannot be uploaded concurrently or reentered from persist", async () => {
  const f = await fixture([...preflight(), reservation(undefined, new Error("lost response"))]);
  const state = {};
  await assert.rejects(f.client.uploadListingImage(request, PNG, state, async () => {
    await assert.rejects(f.client.uploadListingImage(request, PNG, state, noop), errorCode("checkpoint_in_use"));
  }), errorCode("write_outcome_unknown"));
  f.verify();
});

test("invalid PNG and changed requests fail before HTTP dispatch", async () => {
  const f = await fixture([]);
  for (const png of [Buffer.from("not PNG"), PNG.subarray(0, 24)]) await assert.rejects(f.client.uploadListingImage(request, png, {}, noop), errorCode("invalid_png"));
  await assert.rejects(f.client.uploadListingImage({ ...request, fileName: "changed.png" }, PNG, checkpoint("committed"), noop), errorCode("checkpoint_mismatch"));
  f.verify();
});

test("untrusted upload URLs, headers and incomplete chunks cannot dispatch bytes", async t => {
  for (const operations of [
    [{ ...uploadOperation(), requestHeaders: [{ name: "Authorization", value: "Bearer private" }] }],
    [uploadOperation("/upload", 1, PNG.length - 1)],
    [{ ...uploadOperation(), url: "https://blobstore.apple.com.evil.invalid/upload" }],
  ]) await t.test(JSON.stringify(operations), async () => {
    const f = await fixture([...preflight(), reservation(operations)]);
    const state = {};
    await assert.rejects(f.client.uploadListingImage(request, PNG, state, noop), errorCode("upstream_invalid_response"));
    assert.equal(state.imageId, "image1");
    assert.ok(!f.calls.some(call => ["PUT", "PATCH"].includes(call.init.method)));
    f.verify();
  });
});
