import { invalidQuery } from "./errors.js";

export function queryFor(route, search) {
  const values = {}, upstream = new URLSearchParams();
  const allowed = new Map(route.parameters.map(p => [p.name, p]));
  for (const [name, raw] of search) {
    const parameter = allowed.get(name);
    if (!parameter || Object.hasOwn(values, name)) throw invalidQuery(name);
    if (!raw || raw.length > (name === "cursor" ? 8192 : 4096)) throw invalidQuery(name, "Provide a nonempty value within the size limit.");
    const schema = parameter.schema;
    let value = raw;
    if (schema.type === "integer") {
      if (!/^[1-9]\d{0,2}$/.test(raw) || Number(raw) > (schema.maximum ?? 200)) throw invalidQuery(name, "Provide a page size between 1 and 200.");
      value = Number(raw);
    } else if (schema.type === "array") {
      value = raw.split(",");
      if (value.length > 200 || value.some(v => !v || v.length > 256 || (schema.items.enum && !schema.items.enum.includes(v)))) {
        throw invalidQuery(name, "Provide supported comma-separated values.");
      }
    } else if (schema.type === "boolean") {
      if (!["true", "false"].includes(raw)) throw invalidQuery(name, "Provide true or false.");
      value = raw === "true";
    }
    values[name] = value;
    if (parameter.appleName) upstream.set(parameter.appleName, raw);
  }
  for (const parameter of route.parameters) {
    if (!Object.hasOwn(values, parameter.name) && "default" in parameter) {
      values[parameter.name] = parameter.default;
      if (parameter.appleName) upstream.set(parameter.appleName, String(parameter.default));
    }
    if (parameter.required && !Object.hasOwn(values, parameter.name)) throw invalidQuery(parameter.name, "This parameter is required.");
  }
  return { values, upstream };
}

export function canonicalQuery(query) {
  const result = new URLSearchParams(query);
  result.delete("cursor");
  result.sort();
  return result.toString();
}
