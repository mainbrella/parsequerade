import contract from "../api/age-rating-write-contract.json" with { type: "json" };
import { isObject } from "./auth.js";
import { ApiError, invalidUpstream } from "./errors.js";
import { APPLE_ORIGIN } from "./apple.js";
import { flatten, readResource } from "./resources.js";

const invalid = field => new ApiError(400, "invalid_request", "Provide valid age rating attributes.", {
  details: field ? [{ field, code: "invalid_attribute", message: "This age rating attribute or value is unsupported." }] : [],
});

export async function ageRatingInput(request) {
  if (request.headers.get("Content-Type")?.split(";")[0].trim().toLowerCase() !== "application/json") {
    throw new ApiError(415, "unsupported_media_type", "Send age rating attributes as application/json.");
  }
  const tooLarge = () => new ApiError(413, "request_too_large", "Age rating attributes must fit within 8 KiB.");
  if (Number(request.headers.get("Content-Length")) > 8192) throw tooLarge();
  if (!request.body) throw invalid();
  const reader = request.body.getReader();
  const chunks = [];
  let size = 0, timer;
  try {
    const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(new ApiError(408, "request_timeout", "The request body did not arrive in time.")), 5000); });
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
    let attributes;
    try { attributes = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(buffer)); }
    catch { throw invalid(); }
    if (!isObject(attributes) || !Object.keys(attributes).length) throw invalid();
    for (const [name, value] of Object.entries(attributes)) {
      const schema = Object.hasOwn(contract.attributes, name) ? contract.attributes[name] : null;
      if (!schema) throw invalid(name);
      if (value === null) { if (!schema.nullable) throw invalid(name); continue; }
      if (typeof value !== schema.type || (schema.enum && !schema.enum.includes(value))) throw invalid(name);
      if (schema.type === "string" && value.length > 2048) throw invalid(name);
      if (schema.format === "uri") {
        try {
          const url = new URL(value);
          if (!["https:", "http:"].includes(url.protocol) || url.username || url.password) throw new Error();
        } catch { throw invalid(name); }
      }
    }
    return attributes;
  } finally {
    clearTimeout(timer);
    void reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

export async function writeAgeRating(ctx, attributes, transport) {
  // Resolve only through the target app's scoped app-info collection. Never accept a declaration ID from callers.
  const current = await readResource(ctx, transport);
  const payload = await transport.patchAgeRating(ctx.config, ctx.credential,
    new URL(`/v1/ageRatingDeclarations/${current.id}`, APPLE_ORIGIN), ctx.budget,
    { data: { type: "ageRatingDeclarations", id: current.id, attributes } }, ctx.route);
  try {
    if (payload.data?.id !== current.id) throw invalidUpstream();
    return flatten(payload.data, ctx.route, { appId: ctx.params.appId, appInfoId: ctx.params.appInfoId });
  } catch (error) {
    if (!(error instanceof ApiError) || error.code !== "upstream_invalid_response") throw error;
    throw new ApiError(502, "write_outcome_unknown", "Apple returned an invalid update response. Refresh the age rating before trying again.");
  }
}
