// Test helpers for the Access check: RSA keys, signed tokens, a stub key server and the real wasm module.
// Runs in plain Node 22 (Web Crypto and fetch are global).

import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync } from "node:fs";
import { coreFromWasm, type AccessCore, type AccessWasmExports } from "../src/access";

export const TEAM = "acme.cloudflareaccess.com";
export const AUD = "aud-tag-1234";
export const APP_HOST = "canvas.patwie.com";
export const CERTS_URL = `https://${TEAM}/cdn-cgi/access/certs`;
export const NOW_MS = 1_800_000_000_000;
export const NOW_SECS = NOW_MS / 1000;
export const EMAIL = "alice@example.com";
export const CLIENT_ID = "0123456789abcdef.access";

/**
 * The wasm module of `drafftink-worker`, built for Node by `wasm-bindgen --target nodejs` (see ACCESS.md). The
 * folder comes from `ACCESS_WASM_DIR`, else `<repo>/target/access-wasm`.
 */
export function loadCore(): AccessCore {
  const here = dirname(fileURLToPath(import.meta.url));
  const dir = process.env.ACCESS_WASM_DIR ?? resolve(here, "../../../target/access-wasm");
  const entry = resolve(dir, "drafftink_worker.js");
  if (!existsSync(entry)) {
    throw new Error(`The wasm module is missing at ${entry}. Build it first: see apps/drafftink-worker/ACCESS.md.`);
  }
  return coreFromWasm(createRequire(import.meta.url)(entry) as AccessWasmExports);
}

const RSA_PARAMS = { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" } as const;

export interface TestKey {
  readonly kid: string;
  readonly privateKey: CryptoKey;
  /** The public JWK as the certs endpoint serves it. */
  readonly jwk: JsonWebKey & { kid: string };
}

export async function makeKey(kid: string): Promise<TestKey> {
  const pair = await crypto.subtle.generateKey({ ...RSA_PARAMS, modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]) }, true, ["sign", "verify"]);
  const exported = await crypto.subtle.exportKey("jwk", pair.publicKey);
  return { kid, privateKey: pair.privateKey, jwk: { ...exported, kid, alg: "RS256", use: "sig" } };
}

export function b64url(bytes: Uint8Array | string): string {
  const raw = typeof bytes === "string" ? new TextEncoder().encode(bytes) : bytes;
  return Buffer.from(raw).toString("base64url");
}

export type Json = Record<string, unknown>;

/** `header.payload.signature`, signed with RS256 by `key`. */
export async function signRs256(key: TestKey, header: Json, payload: Json): Promise<string> {
  const signingInput = `${b64url(JSON.stringify(header))}.${b64url(JSON.stringify(payload))}`;
  const signature = await crypto.subtle.sign(RSA_PARAMS.name, key.privateKey, new TextEncoder().encode(signingInput));
  return `${signingInput}.${b64url(new Uint8Array(signature))}`;
}

export function personClaims(overrides: Json = {}): Json {
  return {
    iss: `https://${TEAM}`,
    aud: [AUD],
    exp: NOW_SECS + 600,
    iat: NOW_SECS - 60,
    nbf: NOW_SECS - 60,
    email: EMAIL,
    type: "app",
    ...overrides,
  };
}

export function machineClaims(overrides: Json = {}): Json {
  return { iss: `https://${TEAM}`, aud: [AUD], exp: NOW_SECS + 600, common_name: CLIENT_ID, type: "app", ...overrides };
}

export function rs256Header(key: TestKey, overrides: Json = {}): Json {
  return { alg: "RS256", kid: key.kid, typ: "JWT", ...overrides };
}

/** A valid person token signed by `key`. */
export function personToken(key: TestKey, overrides: Json = {}): Promise<string> {
  return signRs256(key, rs256Header(key), personClaims(overrides));
}

/** The classic algorithm-confusion token: HS256, with the public key's bytes as the HMAC secret. */
export async function hs256WithPublicKey(key: TestKey, claims: Json = personClaims()): Promise<string> {
  const header = { alg: "HS256", kid: key.kid, typ: "JWT" };
  const signingInput = `${b64url(JSON.stringify(header))}.${b64url(JSON.stringify(claims))}`;
  const secret = new TextEncoder().encode(JSON.stringify(key.jwk));
  const hmacKey = await crypto.subtle.importKey("raw", secret, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const mac = await crypto.subtle.sign("HMAC", hmacKey, new TextEncoder().encode(signingInput));
  return `${signingInput}.${b64url(new Uint8Array(mac))}`;
}

export type KeyServerMode = "ok" | "http_error" | "network_error" | "garbage" | "no_keys";

/** A stand-in for the certs endpoint. It counts requests and can be told to fail. */
export class KeyServer {
  calls = 0;
  urls: string[] = [];
  mode: KeyServerMode = "ok";

  constructor(public keys: readonly TestKey[]) {}

  readonly fetch: typeof fetch = (input) => {
    this.calls += 1;
    this.urls.push(input instanceof Request ? input.url : String(input));
    switch (this.mode) {
      case "network_error":
        return Promise.reject(new TypeError("network down"));
      case "http_error":
        return Promise.resolve(new Response("nope", { status: 503 }));
      case "garbage":
        return Promise.resolve(new Response("<html>not json</html>", { status: 200 }));
      case "no_keys":
        return Promise.resolve(Response.json({ keys: [], public_cert: { kid: "x", cert: "-----BEGIN CERTIFICATE-----" } }));
      case "ok":
        return Promise.resolve(Response.json({ keys: this.keys.map((key) => key.jwk), public_cert: { kid: "x", cert: "-----BEGIN CERTIFICATE-----" } }));
    }
  };
}

/** A request as Access forwards it: the assertion in the header. */
export function requestWith(token: string | undefined, init: { headers?: Record<string, string>; url?: string } = {}): Request {
  const headers = new Headers(init.headers);
  if (token !== undefined) headers.set("Cf-Access-Jwt-Assertion", token);
  return new Request(init.url ?? `https://${APP_HOST}/r/room1/ws`, { headers });
}
