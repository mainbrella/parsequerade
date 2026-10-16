import { Buffer } from "node:buffer";
import { ApiError } from "./errors.js";
import { ResourceClient } from "./generated.js";
import { core } from "./native.js";
import type {
  AgeRating, AgeRatingUpdate, App, AppUpdate, AppInfoLocalization, AppInfoLocalizationUpdate,
  Version, VersionUpdate, VersionLocalization, VersionLocalizationUpdate, Query,
  ListingImageUpload, ListingImageUploadCheckpoint, ListingImageUploadReceipt,
} from "./generated.js";

export { ApiError } from "./errors.js";
export type { ErrorDetail } from "./errors.js";
export type * from "./generated.js";

export interface ClientOptions {
  issuerId: string;
  keyId: string;
  /** The PKCS#8 PEM contents of the Apple team's .p8 file, not a path. */
  privateKey: string;
  /** Optional trusted fetch implementation. It receives signed Apple requests and upload chunks. */
  fetch?: typeof globalThis.fetch;
}

export type PersistCheckpoint = (snapshot: ListingImageUploadCheckpoint) => void | Promise<void>;
type CoreError = Parameters<typeof ApiError.fromCore>[0];
type HttpEvent = { kind: "http"; id: number; method: string; url: string;
  headers: Record<string, string>; body: string | null; timeoutMs: number; deadlineMs: number; maxResponseBytes: number };
type Event = HttpEvent | { kind: "checkpoint"; id: number; checkpoint: ListingImageUploadCheckpoint }
  | { kind: "result"; value?: unknown; error?: CoreError; checkpoint?: ListingImageUploadCheckpoint };
type UploadOperation = { png: string; checkpoint: ListingImageUploadCheckpoint; persist: PersistCheckpoint;
  pendingPersistence: number; finished: boolean };

const finalizer = new FinalizationRegistry<number>(id => core.clientFree(id));
const activeCheckpoints = new WeakSet<ListingImageUploadCheckpoint>();

function releaseCheckpoint(upload: UploadOperation): void {
  if (upload.finished && upload.pendingPersistence === 0) activeCheckpoints.delete(upload.checkpoint);
}

function invalidCredentials(): ApiError {
  return new ApiError(400, "invalid_credentials", "Provide an issuer ID, key ID and a P-256 PKCS#8 private key.");
}

function encode(value: unknown): string {
  try {
    const encoded = JSON.stringify(value);
    if (encoded !== undefined) return encoded;
  } catch { /* Surface only the facade error; JSON diagnostics can include request data. */ }
  throw new ApiError(400, "invalid_request", "Provide a JSON-compatible request.");
}

function queryPairs(query: Query): [string, string][] {
  if (query instanceof URLSearchParams) return [...query.entries()];
  if (query === null || typeof query !== "object" || Array.isArray(query)) {
    throw new ApiError(400, "invalid_query", "Provide a query object or URLSearchParams.");
  }
  return Object.entries(query).flatMap(([name, value]) => {
    if (value === undefined) return [];
    if (!(typeof value === "string" || typeof value === "boolean" ||
        (typeof value === "number" && Number.isFinite(value)) ||
        (Array.isArray(value) && value.every(item => typeof item === "string")))) {
      throw new ApiError(400, "invalid_query", "The request query is invalid.", {
        details: [{ field: name, code: "invalid_parameter", message: "Provide a supported parameter and value." }],
      });
    }
    return [[name, Array.isArray(value) ? value.join(",") : String(value)]];
  });
}

function resourcePath(template: string, params: Record<string, string>): string {
  return template.replace(/\{(\w+)\}/g, (_, name: string) => {
    const value = params[name];
    const pattern = name === "locale" ? /^[A-Za-z0-9][A-Za-z0-9-]{1,34}$/ : /^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$/;
    if (typeof value !== "string" || !pattern.test(value)) {
      throw new ApiError(400, "invalid_path", "The resource path is invalid.");
    }
    return value;
  });
}

function updateCheckpoint(target: ListingImageUploadCheckpoint, snapshot: ListingImageUploadCheckpoint): void {
  for (const key of Object.keys(target)) delete (target as Record<string, unknown>)[key];
  Object.assign(target, snapshot);
}

/** A local facade using the same execution core as Rust. Reuse it for JWT caching and cursors. */
export class Client extends ResourceClient {
  #id: number;
  #fetch?: typeof globalThis.fetch;
  #operations = new Map<number, AbortController>();

  private constructor(id: number, fetch?: typeof globalThis.fetch) {
    super();
    this.#id = id;
    this.#fetch = fetch;
    finalizer.register(this, id, this);
  }

  /** Validate credentials locally, without a network request or hosted connection. */
  static async create(options: ClientOptions): Promise<Client> {
    if (options === null || typeof options !== "object" ||
        typeof options.issuerId !== "string" || !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i.test(options.issuerId) ||
        typeof options.keyId !== "string" || !/^[A-Z0-9]{10}$/.test(options.keyId) ||
        typeof options.privateKey !== "string" || (options.fetch !== undefined && typeof options.fetch !== "function")) {
      throw invalidCredentials();
    }
    const result = JSON.parse(core.clientNewJson(encode({ issuerId: options.issuerId,
      keyId: options.keyId, privateKey: options.privateKey, customTransport: options.fetch !== undefined })));
    if (result.error) throw ApiError.fromCore(result.error);
    return new Client(result.client, options.fetch);
  }

  /** Release native resources and cancel active operations. Closing twice is harmless. */
  close(): void {
    if (this.#id === 0) return;
    for (const [operation, controller] of this.#operations) {
      core.cancel(operation);
      controller.abort();
    }
    core.clientFree(this.#id);
    this.#id = 0;
    finalizer.unregister(this);
  }

  /** Read an implemented facade path, e.g. /apps/123. */
  async get<T = unknown>(path: string, query: Query = {}): Promise<T> {
    if (typeof path !== "string") throw new ApiError(400, "invalid_path", "The resource path is invalid.");
    return this.#run("get", [path, queryPairs(query)]) as Promise<T>;
  }

  protected async getResource<T>(template: string, params: Record<string, string>, query: Query): Promise<T> {
    return this.get<T>(resourcePath(template, params), query);
  }

  /** Resolve ownership and send one PATCH. Uncertain writes are never retried. */
  async updateAgeRating(appId: string, appInfoId: string, attributes: AgeRatingUpdate): Promise<AgeRating> {
    resourcePath("/apps/{appId}/app-infos/{appInfoId}/age-rating", { appId, appInfoId });
    return this.#run("update_age_rating", [appId, appInfoId, attributes]) as Promise<AgeRating>;
  }

  /** Update editable Store metadata on an app, version, or localization. */
  async updateMetadata<T = App | Version | AppInfoLocalization | VersionLocalization>(path: string,
    attributes: AppUpdate | VersionUpdate | AppInfoLocalizationUpdate | VersionLocalizationUpdate): Promise<T> {
    return this.#run("update_metadata", [path, attributes]) as Promise<T>;
  }

  async createVersion(appId: string, platform: "IOS" | "MAC_OS" | "TV_OS" | "VISION_OS", versionString: string): Promise<Version> {
    return this.#run("create_version", [appId, platform, versionString]) as Promise<Version>;
  }

  /** Attach an eligible build to its matching draft version and read back to verify. */
  async attachBuild(appId: string, versionId: string, buildId: string): Promise<Version> {
    return this.#run("attach_build", [appId, versionId, buildId]) as Promise<Version>;
  }

  /** Read Apple's live asset specifications and placement groups. */
  async listingImageReferenceData(): Promise<Record<string, unknown>> {
    return this.#run("listing_image_reference_data", []) as Promise<Record<string, unknown>>;
  }

  /** Upload or resume one PNG. Save each independent snapshot durably before resolving persist. */
  async uploadListingImage(request: ListingImageUpload, png: Uint8Array,
    checkpoint: ListingImageUploadCheckpoint, persist: PersistCheckpoint): Promise<ListingImageUploadReceipt> {
    if (checkpoint === null || typeof checkpoint !== "object" || Array.isArray(checkpoint) || typeof persist !== "function") {
      throw new TypeError("checkpoint must be an object and persist must be a function");
    }
    if (!(png instanceof Uint8Array)) throw new TypeError("png must be a Uint8Array");
    if (activeCheckpoints.has(checkpoint)) throw new ApiError(409, "checkpoint_in_use", "This checkpoint already has an upload in progress.");
    const upload: UploadOperation = { png: "", checkpoint, persist, pendingPersistence: 0, finished: false };
    activeCheckpoints.add(checkpoint);
    try {
      upload.png = Buffer.from(png).toString("base64");
      return await this.#run("upload_listing_image", [request, checkpoint], upload) as ListingImageUploadReceipt;
    } finally {
      upload.finished = true;
      releaseCheckpoint(upload);
    }
  }

  async #run(operation: string, args: unknown[], upload?: UploadOperation): Promise<unknown> {
    if (this.#id === 0) throw new ApiError(409, "client_closed", "This client is closed.");
    // Snapshot inputs before the first await so caller mutation cannot change a dispatched write.
    const started = JSON.parse(core.startJson(this.#id, encode({ operation, args, png: upload?.png })));
    if (started.error) throw ApiError.fromCore(started.error);
    const id: number = started.operation;
    const controller = new AbortController();
    this.#operations.set(id, controller);
    let callbackFailed = false;
    let callbackError: unknown;
    try {
      for (;;) {
        const event: Event | null = JSON.parse(await core.pollJson(id, 50));
        if (!event) continue;
        if (event.kind === "result") {
          if (upload && event.checkpoint) updateCheckpoint(upload.checkpoint, event.checkpoint);
          if (callbackFailed) throw callbackError;
          if (event.error) throw ApiError.fromCore(event.error);
          return event.value;
        }
        if (event.kind === "http") {
          const result = await this.#http(event, controller.signal);
          core.respondJson(id, event.id, encode(result));
        } else if (upload) {
          updateCheckpoint(upload.checkpoint, event.checkpoint);
          if (controller.signal.aborted) {
            core.respondJson(id, event.id, '{"error":"checkpoint_persistence_failed"}');
            continue;
          }
          let cancelPersistence = () => {};
          try {
            // Separate objects keep persisted snapshots independent from live and later states.
            const aborted = new Promise<never>((_, reject) => {
              cancelPersistence = () => reject(new Error());
              controller.signal.addEventListener("abort", cancelPersistence, { once: true });
            });
            upload.pendingPersistence++;
            const persisted = Promise.resolve().then(() => {
              if (controller.signal.aborted) throw new Error();
              return upload.persist(JSON.parse(encode(event.checkpoint)));
            }).finally(() => {
              upload.pendingPersistence--;
              releaseCheckpoint(upload);
            });
            const returned = await Promise.race([
              aborted,
              persisted,
            ]);
            if (returned !== undefined) throw new TypeError("persist must resolve without a value after durably saving the checkpoint");
            core.respondJson(id, event.id, '{"ok":true}');
          } catch (error) {
            if (!controller.signal.aborted) {
              callbackFailed = true;
              callbackError = error;
            }
            core.respondJson(id, event.id, '{"error":"checkpoint_persistence_failed"}');
          } finally {
            controller.signal.removeEventListener("abort", cancelPersistence);
          }
        }
      }
    } finally {
      core.operationFree(id);
      this.#operations.delete(id);
      controller.abort();
    }
  }

  async #http(event: HttpEvent, signal: AbortSignal): Promise<unknown> {
    if (signal.aborted) return { error: "failed" };
    const remaining = Math.min(event.timeoutMs, event.deadlineMs - Date.now());
    if (!(remaining > 0)) return { error: "timeout" };
    const controller = new AbortController();
    let timedOut = false;
    const timeout = setTimeout(() => { timedOut = true; controller.abort(); }, remaining);
    const abort = () => controller.abort();
    signal.addEventListener("abort", abort, { once: true });
    if (signal.aborted) controller.abort();
    try {
      const headers = { ...event.headers };
      if (headers.authorization !== undefined) {
        headers.Authorization = headers.authorization;
        delete headers.authorization;
      }
      const transfer = async () => {
        const body = event.body === null ? undefined : Buffer.from(event.body, "base64");
        if (controller.signal.aborted) return { error: timedOut ? "timeout" : "failed" };
        if (Date.now() >= event.deadlineMs) return { error: "timeout" };
        const response = await this.#fetch!(event.url, { method: event.method, headers,
          body,
          redirect: "manual", signal: controller.signal });
        if (!(response instanceof Response)) return { error: "invalid_response" };
        const chunks: Uint8Array[] = [];
        let size = 0;
        const reader = response.body?.getReader();
        if (reader) {
          const cancelBody = () => { void reader.cancel().catch(() => {}); };
          controller.signal.addEventListener("abort", cancelBody, { once: true });
          if (controller.signal.aborted) cancelBody();
          try {
            for (;;) {
              const item = await reader.read();
              if (item.done) break;
              size += item.value.byteLength;
              if (size > event.maxResponseBytes) {
                await reader.cancel();
                return { error: "invalid_response" };
              }
              chunks.push(item.value);
            }
          } finally {
            controller.signal.removeEventListener("abort", cancelBody);
            reader.releaseLock();
          }
        }
        const bytes = new Uint8Array(size);
        let offset = 0;
        for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
        return { status: response.status, headers: Object.fromEntries(response.headers), body: Buffer.from(bytes).toString("base64") };
      };
      return await Promise.race([transfer(), new Promise<never>((_, reject) => {
        if (controller.signal.aborted) reject(new Error());
        else controller.signal.addEventListener("abort", () => reject(new Error()), { once: true });
      })]);
    } catch {
      return { error: timedOut ? "timeout" : "failed" };
    } finally {
      clearTimeout(timeout);
      signal.removeEventListener("abort", abort);
    }
  }
}
