import manifest from "../api/read-contracts.json" with { type: "json" };
import { identifier, isObject } from "./auth.js";
import { ApiError, invalidUpstream, notFound } from "./errors.js";
import { POLICY } from "./apple.js";
import { appleUrl, nextPage, pageHash, traversalError } from "./pagination.js";
import { canonicalQuery } from "./query.js";
import { sha256 } from "./crypto.js";

export const routes = manifest.routes;
const byHandler = new Map(routes.map(route => [route.handler, route]));
const patterns = routes.map(route => ({ route, names: [...route.route.matchAll(/\{(\w+)\}/g)].map(m => m[1]),
  pattern: new RegExp("^" + route.route.replace(/\{\w+\}/g, "([^/]+)") + "$"), }));

export function matchRoute(path) {
  for (const entry of patterns) {
    const match = entry.pattern.exec(path);
    if (!match) continue;
    const params = {};
    for (let i = 0; i < entry.names.length; i++) {
      const name = entry.names[i];
      let value;
      try { value = decodeURIComponent(match[i + 1]); } catch { throw new ApiError(400, "invalid_path", "The resource path is invalid."); }
      if (!(name === "locale" ? /^[A-Za-z0-9][A-Za-z0-9-]{1,34}$/ : identifier).test(value)) throw new ApiError(400, "invalid_path", "The resource path is invalid.");
      params[name] = value;
    }
    return { route: entry.route, params };
  }
  return null;
}

function resource(value, type) {
  if (!isObject(value) || value.type !== type || typeof value.id !== "string" || !identifier.test(value.id) ||
      (value.attributes !== undefined && !isObject(value.attributes)) ||
      (value.relationships !== undefined && !isObject(value.relationships))) throw invalidUpstream();
  return value;
}

function checkParent(value, relation, id, type) {
  const data = value.relationships?.[relation]?.data;
  if (data !== undefined && (!isObject(data) || data.id !== id || data.type !== type)) throw invalidUpstream();
}

export function flatten(value, route, context = {}, fields) {
  resource(value, route.type);
  const result = { id: value.id, ...context };
  for (const [name, schema] of Object.entries(route.attributes)) {
    if (fields && !fields.includes(name)) continue;
    if (!Object.hasOwn(value.attributes || {}, name)) continue;
    const item = value.attributes[name];
    if (item !== null && typeof item !== schema.type) throw invalidUpstream();
    result[name] = item;
  }
  for (const [name, relationship] of Object.entries(route.relationships)) {
    const data = value.relationships?.[name]?.data;
    if (data === undefined) continue;
    if (data !== null && (!isObject(data) || data.type !== relationship.type || typeof data.id !== "string" || !identifier.test(data.id))) throw invalidUpstream();
    result[relationship.field] = data === null ? null : data.id;
  }
  return result;
}

function pageData(payload, type, limit) {
  if (!Array.isArray(payload.data) || payload.data.length > limit || !isObject(payload.links)) throw invalidUpstream();
  const ids = new Set();
  for (const item of payload.data) {
    resource(item, type);
    if (ids.has(item.id)) throw invalidUpstream();
    ids.add(item.id);
  }
  return payload.data;
}

export async function readResource(ctx, transport) {
  const { route, params: p, config, credential, auth, query, codec } = ctx;
  const get = (url, contract = route) => transport.get(config, credential, url, ctx.budget, contract);
  const info = route.handler.startsWith("appInfo") || route.handler === "ageRating";
  const parentId = info ? p.appInfoId : p.versionId;
  const parentType = info ? "appInfos" : "appStoreVersions";
  const parentPath = info ? "appInfos" : "appStoreVersions";
  const context = route.handler === "app" || route.handler === "apps" ? {} : {
    appId: p.appId, ...(parentId && !["appInfo", "version"].includes(route.handler) ? { [info ? "appInfoId" : "versionId"]: parentId } : {}),
  };

  async function traverse(path, parameters, type, visit, contract) {
    const initial = appleUrl(path, parameters);
    let url = initial;
    const seen = new Set();
    for (let page = 0; page < POLICY.resolverPages; page++) {
      const hash = await pageHash(url);
      if (seen.has(hash)) throw traversalError();
      seen.add(hash);
      const payload = await get(url, contract);
      const items = pageData(payload, type, Number(parameters.get("limit")));
      if (visit(items)) return;
      url = nextPage(payload.links?.next, initial);
      if (!url) return;
    }
    throw traversalError();
  }

  if (parentId) {
    // Membership in the app's scoped collection proves ownership even when Apple omits linkage.
    let found = false;
    const parameters = new URLSearchParams({ limit: "200" });
    if (!info) parameters.set("filter[id]", parentId);
    await traverse(`/v1/apps/${p.appId}/${parentPath}`, parameters, parentType, items => {
      for (const item of items) checkParent(item, "app", p.appId, "apps");
      found = items.some(item => item.id === parentId);
      return found;
    }, byHandler.get(info ? "appInfos" : "versions"));
    if (!found) throw notFound();
  }

  let path;
  switch (route.handler) {
    case "apps": path = "/v1/apps"; break;
    case "app": path = `/v1/apps/${p.appId}`; break;
    case "appInfos": path = `/v1/apps/${p.appId}/appInfos`; break;
    case "appInfo": path = `/v1/appInfos/${parentId}`; break;
    case "versions": path = `/v1/apps/${p.appId}/appStoreVersions`; break;
    case "version": path = `/v1/appStoreVersions/${parentId}`; break;
    case "appInfoLocalizations": path = `/v1/appInfos/${parentId}/appInfoLocalizations`; break;
    case "versionLocalizations": path = `/v1/appStoreVersions/${parentId}/appStoreVersionLocalizations`; break;
    case "ageRating": path = `/v1/appInfos/${parentId}/ageRatingDeclaration`; break;
    case "reviewContact": path = `/v1/appStoreVersions/${parentId}/appStoreReviewDetail`; break;
    case "appInfoLocalization":
    case "versionLocalization": {
      const localizations = info ? "appInfoLocalizations" : "appStoreVersionLocalizations";
      const parameters = new URLSearchParams({ limit: "200", "filter[locale]": p.locale, [`fields[${localizations}]`]: "locale" });
      let selected;
      await traverse(`/v1/${parentPath}/${parentId}/${localizations}`, parameters, localizations, items => {
        for (const item of items) {
          checkParent(item, info ? "appInfo" : "appStoreVersion", parentId, parentType);
          if (typeof item.attributes?.locale !== "string") throw invalidUpstream();
          if (item.attributes.locale !== p.locale) continue;
          if (selected) throw new ApiError(409, "ambiguous_selection", "More than one localization matches this locale.");
          selected = item;
        }
        return false; // Prove uniqueness across every page, including the final page.
      }, byHandler.get(info ? "appInfoLocalizations" : "versionLocalizations"));
      if (!selected) throw notFound("localization_not_found");
      path = `/v1/${localizations}/${selected.id}`;
      if (query.values.fields && !query.values.fields.includes("locale")) {
        query.upstream.set(`fields[${localizations}]`, [...query.values.fields, "locale"].join(","));
      }
      break;
    }
    default: throw notFound("route_not_found");
  }

  if (route.handler === "apps" && auth.grant.appIds !== "*") {
    const requested = query.values.id;
    const permitted = [...new Set(auth.grant.appIds)].filter(id => !requested || requested.includes(id)).sort();
    // Validate incoming cursors below even for an empty scope; never skip authorization binding.
    query.upstream.set("filter[id]", permitted.join(","));
  }
  const initial = appleUrl(path, query.upstream);
  if (!route.collection) {
    const payload = await get(initial);
    if (payload.data === null) throw notFound(route.handler === "reviewContact" ? "review_contact_not_found" : "resource_not_found");
    resource(payload.data, route.type);
    const expectedId = route.handler === "app" ? p.appId : ["appInfo", "version"].includes(route.handler) ? parentId : null;
    if (expectedId && payload.data.id !== expectedId) throw invalidUpstream();
    if (route.handler === "appInfo" || route.handler === "version") checkParent(payload.data, "app", p.appId, "apps");
    if (p.locale && payload.data.attributes?.locale !== p.locale) throw invalidUpstream();
    if (p.locale) checkParent(payload.data, info ? "appInfo" : "appStoreVersion", parentId, parentType);
    if (route.handler === "reviewContact") checkParent(payload.data, "appStoreVersion", parentId, parentType);
    if (p.locale && payload.data.id !== path.split("/").at(-1)) throw invalidUpstream();
    return flatten(payload.data, route, context, query.values.fields);
  }

  const binding = await sha256(JSON.stringify([config.connectionId, credential.fingerprint,
    auth.authorizationRevision, ctx.resourcePath, canonicalQuery(initial.searchParams)]));
  const prior = query.values.cursor ? await codec.decode(query.values.cursor, binding) : null;
  const seen = [...(prior?.seen || [])];
  if (seen.length >= POLICY.cursorPages) throw traversalError();
  const url = new URL(initial);
  if (prior) url.searchParams.set("cursor", prior.next);
  url.searchParams.sort();
  if (route.handler === "apps" && auth.grant.appIds !== "*" && !query.upstream.get("filter[id]")) return { items: [], nextCursor: null };
  const hash = await pageHash(url);
  if (seen.includes(hash)) throw traversalError();
  seen.push(hash);
  const payload = await get(url);
  const items = pageData(payload, route.type, query.values.limit).map(item => {
    if (route.handler === "apps" && auth.grant.appIds !== "*" && !auth.grant.appIds.includes(item.id)) throw invalidUpstream();
    if (["appInfos", "versions"].includes(route.handler)) checkParent(item, "app", p.appId, "apps");
    if (["appInfoLocalizations", "versionLocalizations"].includes(route.handler)) checkParent(item, info ? "appInfo" : "appStoreVersion", parentId, parentType);
    return flatten(item, route, context, query.values.fields);
  });
  const next = nextPage(payload.links?.next, initial);
  if (next && seen.includes(await pageHash(next))) throw traversalError();
  return { items, nextCursor: next ? await codec.encode(binding, next.searchParams.get("cursor"), seen, prior?.expires) : null };
}
