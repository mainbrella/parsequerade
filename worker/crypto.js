const encoder = new TextEncoder();

export function base64url(bytes) {
  return btoa(String.fromCharCode(...bytes)).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

export function unbase64url(value) {
  if (!/^[A-Za-z0-9_-]+$/.test(value)) throw new Error("Invalid base64url");
  return Uint8Array.from(atob(value.replaceAll("-", "+").replaceAll("_", "/")), c => c.charCodeAt(0));
}

export function bytes(value) { return encoder.encode(value); }

export async function digest(value) {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", bytes(value)));
}

export async function sha256(value) {
  return Array.from(await digest(value), b => b.toString(16).padStart(2, "0")).join("");
}

export function equal(a, b) {
  if (a.length !== b.length) return false;
  let difference = 0;
  for (let i = 0; i < a.length; i++) difference |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return difference === 0;
}
