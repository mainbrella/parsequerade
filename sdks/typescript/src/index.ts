import { createAppleTransport } from "../../../worker/apple.js";
import { isObject } from "../../../worker/auth.js";
import { cursorCodec } from "../../../worker/pagination.js";
import { queryFor } from "../../../worker/query.js";
import { matchRoute, readResource } from "../../../worker/resources.js";
import { validateAgeRatingAttributes, writeAgeRating } from "../../../worker/writes.js";
import { ApiError } from "./errors.js";
import { ResourceClient } from "./generated.js";
import type { AgeRating, AgeRatingUpdate, Query } from "./generated.js";

export { ApiError } from "./errors.js";
export type { ErrorDetail } from "./errors.js";
export type * from "./generated.js";

export interface ClientOptions {
  issuerId: string;
  keyId: string;
  /** The PKCS#8 PEM contents of the Apple team key's .p8 file, not a path. */
  privateKey: string;
  /** Optional trusted fetch implementation. It receives the signed Apple requests. */
  fetch?: typeof globalThis.fetch;
}

function invalidCredentials(): ApiError {
  return new ApiError(400, "invalid_credentials", "Provide an issuer ID, key ID and a P-256 PKCS#8 private key.");
}

function searchParams(query: Query): URLSearchParams {
  if (query instanceof URLSearchParams) return new URLSearchParams(query);
  if (!isObject(query)) throw new ApiError(400, "invalid_query", "Provide a query object or URLSearchParams.");
  const result = new URLSearchParams();
  for (const [name, value] of Object.entries(query)) {
    if (value === undefined) continue;
    if (!(typeof value === "string" || typeof value === "number" || typeof value === "boolean" ||
        (Array.isArray(value) && value.every(item => typeof item === "string")))) {
      throw new ApiError(400, "invalid_query", "The request query is invalid.", {
        details: [{ field: name, code: "invalid_parameter", message: "Provide a string, number, boolean or string array." }],
      });
    }
    result.append(name, Array.isArray(value) ? value.join(",") : String(value));
  }
  return result;
}

function resourcePath(template: string, params: Record<string, string>): string {
  let path: string;
  try {
    path = template.replace(/\{(\w+)\}/g, (_, name: string) => {
      const value = params[name];
      if (typeof value !== "string") throw new Error();
      return encodeURIComponent(value);
    });
  } catch { throw new ApiError(400, "invalid_path", "The resource path is invalid."); }
  if (matchRoute(path)?.route.route !== template) throw new ApiError(400, "invalid_path", "The resource path is invalid.");
  return path;
}

interface ClientState {
  config: { connectionId: string; issuerId: string; keyId: string; credentialRevision: string; privateKey: string };
  transport: ReturnType<typeof createAppleTransport>;
  credential: Awaited<ReturnType<ReturnType<typeof createAppleTransport>["credential"]>>;
  codec: Awaited<ReturnType<typeof cursorCodec>>;
}

/** A local facade backed by an Apple team key. Reuse it for JWT caching and cursors. */
export class Client extends ResourceClient {
  #state: ClientState;

  private constructor(state: ClientState) {
    super();
    this.#state = state;
  }

  /** Validate the key locally. Makes no network requests and stores no hosted connection. */
  static async create(options: ClientOptions): Promise<Client> {
    if (!isObject(options) || typeof options.issuerId !== "string" ||
        !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i.test(options.issuerId) ||
        typeof options.keyId !== "string" || !/^[A-Z0-9]{10}$/.test(options.keyId) ||
        typeof options.privateKey !== "string" || (options.fetch !== undefined && typeof options.fetch !== "function")) {
      throw invalidCredentials();
    }
    const transport = createAppleTransport(options.fetch ? { fetch: options.fetch } : {});
    const config = {
      connectionId: crypto.randomUUID(), issuerId: options.issuerId, keyId: options.keyId,
      credentialRevision: "1", privateKey: options.privateKey,
    };
    let credential;
    try { credential = await transport.credential(config); }
    catch { throw invalidCredentials(); }
    const codec = await cursorCodec(crypto.getRandomValues(new Uint8Array(32)));
    return new Client({ config, transport, credential, codec });
  }

  /** Read an implemented facade path, e.g. /apps/123. URLs and hosted prefixes are unsupported. */
  async get<T = unknown>(path: string, query: Query = {}): Promise<T> {
    return this.#run(path, query) as Promise<T>;
  }

  protected async getResource<T>(template: string, params: Record<string, string>, query: Query): Promise<T> {
    return this.get<T>(resourcePath(template, params), query);
  }

  /** Resolve the app's declaration and send one Apple PATCH. Writes are never retried. */
  async updateAgeRating(appId: string, appInfoId: string, attributes: AgeRatingUpdate): Promise<AgeRating> {
    const path = resourcePath("/apps/{appId}/app-infos/{appInfoId}/age-rating", { appId, appInfoId });
    // Snapshot JSON input so validation and dispatch use the same values across awaited lookups.
    let snapshot: unknown;
    try { snapshot = JSON.parse(JSON.stringify(attributes)); }
    catch { throw new ApiError(400, "invalid_request", "Provide valid age rating attributes."); }
    validateAgeRatingAttributes(snapshot);
    return this.#run(path, {}, snapshot) as Promise<AgeRating>;
  }

  async #run(path: string, query: Query, attributes?: unknown): Promise<unknown> {
    if (typeof path !== "string" || path.length > 12288) throw new ApiError(400, "invalid_path", "The resource path is invalid.");
    const match = matchRoute(path);
    if (!match) throw new ApiError(404, "route_not_found", "The requested route was not found.");
    const parsed = queryFor(match.route, searchParams(query));
    const { config, credential, codec, transport } = this.#state;
    const context = {
      ...match, config, credential, codec, query: parsed, resourcePath: path,
      auth: { grant: { appIds: "*" }, authorizationRevision: "direct" },
      budget: transport.budget(),
    };
    return attributes === undefined ? readResource(context, transport) : writeAgeRating(context, attributes, transport);
  }
}
