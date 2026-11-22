// The Worker side of the Cloudflare Access JWT check (docs/design/collab-e2e.md, section 19.5).
//
// The Rust module `access_jwt` (compiled to wasm) parses the token and checks its claims. This file does the
// parts that need the network, a clock and Web Crypto: it fetches and caches the signing keys, verifies the RS256
// signature with `crypto.subtle`, builds the 401 response and strips the identity before a request goes on.
//
// It uses nothing but `fetch`, `crypto.subtle`, `Request`, `Response` and `Headers`, so it also runs in Node 22.
// It never logs or returns a token or an email address.

// ---------------------------------------------------------------------------------------------------------------
// The wasm side, as this file sees it
// ---------------------------------------------------------------------------------------------------------------

/** The two stable words in the body of a refusal. */
export type AccessCode = "access_required" | "access_invalid";

/** Who a verified token belongs to. The address or client id itself is never handed to TypeScript. */
export type IdentityKind = "person" | "machine";

/** A refusal as the wasm module describes it: stable code, log word and plain-words text. */
export interface RefusalView {
  readonly code: AccessCode;
  readonly reason: string;
  readonly message: string;
}

/** The result of `parse`: the key id, the signed bytes and the signature, or a refusal. */
export type ParseResult =
  | { readonly ok: true; readonly kid: string; readonly signingInput: Uint8Array; readonly signature: Uint8Array }
  | { readonly ok: false; readonly code: AccessCode; readonly reason: string };

/** The result of `checkClaims`. */
export type ClaimsResult =
  | { readonly ok: true; readonly identity: IdentityKind }
  | { readonly ok: false; readonly code: AccessCode; readonly reason: string };

/**
 * The Rust side. The Worker builds it from the wasm exports with {@link coreFromWasm}; a test can pass another
 * implementation.
 */
export interface AccessCore {
  /** Splits the token and refuses every algorithm but RS256. Does not check the signature. */
  parse(token: string): ParseResult;
  /** Checks issuer, audience and times. Call it only after the signature is verified. */
  checkClaims(token: string, nowSecs: number, teamDomain: string, aud: string): ClaimsResult;
  /** The code and the plain-words text for a refusal named by its log word. */
  refusal(reason: string, appHostname: string): RefusalView;
}

/** The exports of the wasm module of `drafftink-worker` that this file uses. */
export interface AccessWasmExports {
  access_parse(token: string): unknown;
  access_check_claims(token: string, nowSecs: number, teamDomain: string, aud: string): unknown;
  access_refusal(reason: string, appHostname: string): unknown;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isCode(value: unknown): value is AccessCode {
  return value === "access_required" || value === "access_invalid";
}

function readParse(value: unknown): ParseResult {
  if (!isRecord(value)) throw new TypeError("access_parse: unexpected result");
  if (value.ok === true && typeof value.kid === "string" && value.signingInput instanceof Uint8Array && value.signature instanceof Uint8Array) {
    return { ok: true, kid: value.kid, signingInput: value.signingInput, signature: value.signature };
  }
  if (value.ok === false && isCode(value.code) && typeof value.reason === "string") {
    return { ok: false, code: value.code, reason: value.reason };
  }
  throw new TypeError("access_parse: unexpected result");
}

function readClaims(value: unknown): ClaimsResult {
  if (!isRecord(value)) throw new TypeError("access_check_claims: unexpected result");
  if (value.ok === true && (value.identity === "person" || value.identity === "machine")) {
    return { ok: true, identity: value.identity };
  }
  if (value.ok === false && isCode(value.code) && typeof value.reason === "string") {
    return { ok: false, code: value.code, reason: value.reason };
  }
  throw new TypeError("access_check_claims: unexpected result");
}

function readRefusal(value: unknown): RefusalView {
  if (isRecord(value) && isCode(value.code) && typeof value.reason === "string" && typeof value.message === "string") {
    return { code: value.code, reason: value.reason, message: value.message };
  }
  throw new TypeError("access_refusal: unexpected result");
}

/** Wraps the wasm exports. Every result is checked for its shape, and a wrong shape throws (the caller fails closed). */
export function coreFromWasm(wasm: AccessWasmExports): AccessCore {
  return {
    parse: (token) => readParse(wasm.access_parse(token)),
    checkClaims: (token, nowSecs, teamDomain, aud) => readClaims(wasm.access_check_claims(token, nowSecs, teamDomain, aud)),
    refusal: (reason, appHostname) => readRefusal(wasm.access_refusal(reason, appHostname)),
  };
}

// ---------------------------------------------------------------------------------------------------------------
// Outcomes and the 401 response
// ---------------------------------------------------------------------------------------------------------------

/** The request carries a valid assertion. */
export interface AccessGranted {
  readonly ok: true;
  readonly identity: IdentityKind;
}

/** The request is refused. */
export interface AccessDenied {
  readonly ok: false;
  readonly refusal: RefusalView;
}

export type AccessOutcome = AccessGranted | AccessDenied;

/**
 * The answer when the wasm module itself fails (it threw, or it was not loaded). The text is the one the Rust side
 * gives every `access_invalid` refusal; a test keeps the two equal.
 */
export const INTERNAL_REFUSAL: RefusalView = {
  code: "access_invalid",
  reason: "internal",
  message: "Your sign-in is not valid for this server or has expired. Reload the page and sign in again.",
};

/** The 401 response of section 19.5: a small JSON body with a plain message. Never a redirect, never empty. */
export function denied(outcome: AccessDenied): Response {
  const { code, message } = outcome.refusal;
  return new Response(JSON.stringify({ error: code, message }), {
    status: 401,
    headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" },
  });
}

// ---------------------------------------------------------------------------------------------------------------
// Stripping the identity
// ---------------------------------------------------------------------------------------------------------------

/** Every header with this prefix is removed: the assertion, the email, `CF-Access-Client-*`, `cf-access-token`. */
const ACCESS_HEADER_PREFIX = "cf-access-";

/** The cookies Access sets on the application's hostname. Other cookies are kept. */
const ACCESS_COOKIES: ReadonlySet<string> = new Set(["CF_Authorization", "CF_AppSession", "CF_Binding"]);

function cookieName(pair: string): string {
  const equals = pair.indexOf("=");
  return (equals < 0 ? pair : pair.slice(0, equals)).trim();
}

/**
 * A copy of the request without any Access header or cookie, to hand to the Durable Object or R2 so the relay never
 * learns who connected. The body, method and WebSocket upgrade are untouched.
 */
export function stripIdentity(request: Request): Request {
  const headers = new Headers(request.headers);
  for (const name of [...headers.keys()]) {
    if (name.toLowerCase().startsWith(ACCESS_HEADER_PREFIX)) headers.delete(name);
  }
  const cookie = headers.get("cookie");
  if (cookie !== null) {
    const kept = cookie.split(";").filter((pair) => !ACCESS_COOKIES.has(cookieName(pair)));
    if (kept.length === 0) {
      headers.delete("cookie");
    } else {
      headers.set("cookie", kept.map((pair) => pair.trim()).join("; "));
    }
  }
  return new Request(request, { headers });
}

// ---------------------------------------------------------------------------------------------------------------
// The signing keys
// ---------------------------------------------------------------------------------------------------------------

/** How long a fetched key set is trusted. The docs give no TTL; Access rotates keys every six weeks. */
export const KEY_SET_TTL_MS = 60 * 60 * 1000;
/** The least time between two refetches caused by an unknown `kid`. */
export const UNKNOWN_KID_REFETCH_MS = 60 * 1000;
/** After a failed fetch the next request may try again after this long, never later. */
export const FETCH_FAILURE_BACKOFF_MS = 5 * 1000;
const FETCH_TIMEOUT_MS = 3000;
const MAX_KEY_SET_CHARS = 64 * 1024;
const MAX_KEYS = 16;

const RSA_VERIFY = { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" } as const;

type KeyLookup = { readonly key: CryptoKey } | { readonly missing: "unknown_kid" | "keys_unavailable" };

interface RsaJwk {
  readonly kid: string;
  readonly n: string;
  readonly e: string;
}

/** A JWK of the key set that is an RSA signing key we can use, or `undefined`. */
function readRsaJwk(value: unknown): RsaJwk | undefined {
  if (!isRecord(value)) return undefined;
  const { kty, kid, n, e, alg, use } = value;
  if (kty !== "RSA" || typeof kid !== "string" || typeof n !== "string" || typeof e !== "string") return undefined;
  if (alg !== undefined && alg !== "RS256") return undefined;
  if (use !== undefined && use !== "sig") return undefined;
  return { kid, n, e };
}

/** The keys of one certs document (`{ keys: [...] }`). Keys that cannot be imported are left out. */
async function importKeySet(document: unknown): Promise<Map<string, CryptoKey>> {
  const keys = new Map<string, CryptoKey>();
  if (!isRecord(document) || !Array.isArray(document.keys)) return keys;
  for (const entry of document.keys.slice(0, MAX_KEYS) as unknown[]) {
    const jwk = readRsaJwk(entry);
    if (jwk === undefined) continue;
    try {
      const key = await crypto.subtle.importKey("jwk", { kty: "RSA", n: jwk.n, e: jwk.e, alg: "RS256", ext: true }, RSA_VERIFY, false, ["verify"]);
      keys.set(jwk.kid, key);
    } catch {
      // A key that does not import is as good as absent.
    }
  }
  return keys;
}

/** The key set of one team: fetched at most once per hour, once per minute on an unknown `kid`, one fetch at a time. */
class KeyStore {
  private keys = new Map<string, CryptoKey>();
  private loadedAtMs: number | undefined;
  private lastAttemptMs: number | undefined;
  private lastFailureMs: number | undefined;
  private inFlight: Promise<boolean> | undefined;

  constructor(
    private readonly certsUrl: string,
    private readonly fetchFn: typeof fetch,
  ) {}

  async find(kid: string, nowMs: number): Promise<KeyLookup> {
    const fresh = this.loadedAtMs !== undefined && nowMs - this.loadedAtMs < KEY_SET_TTL_MS;
    if (fresh) {
      const known = this.keys.get(kid);
      if (known !== undefined) return { key: known };
      // An unknown kid may mean a rotation. Look again, but not more than once a minute.
      if (this.lastAttemptMs !== undefined && nowMs - this.lastAttemptMs < UNKNOWN_KID_REFETCH_MS) {
        return { missing: "unknown_kid" };
      }
    } else if (this.lastFailureMs !== undefined && nowMs - this.lastFailureMs < FETCH_FAILURE_BACKOFF_MS) {
      return { missing: "keys_unavailable" };
    }
    if (!(await this.load(nowMs))) return { missing: "keys_unavailable" };
    const found = this.keys.get(kid);
    return found === undefined ? { missing: "unknown_kid" } : { key: found };
  }

  private load(nowMs: number): Promise<boolean> {
    if (this.inFlight === undefined) {
      this.lastAttemptMs = nowMs;
      this.inFlight = this.fetchKeys()
        .then((keys) => {
          if (keys === undefined) {
            this.lastFailureMs = nowMs;
            return false;
          }
          this.keys = keys;
          this.loadedAtMs = nowMs;
          this.lastFailureMs = undefined;
          return true;
        })
        .finally(() => {
          this.inFlight = undefined;
        });
    }
    return this.inFlight;
  }

  /** The imported keys, or `undefined` when the fetch failed or the document has no usable key. */
  private async fetchKeys(): Promise<Map<string, CryptoKey> | undefined> {
    try {
      const response = await this.fetchFn(this.certsUrl, {
        headers: { accept: "application/json" },
        redirect: "error",
        signal: AbortSignal.timeout(FETCH_TIMEOUT_MS),
      });
      if (!response.ok) return undefined;
      const text = await response.text();
      if (text.length > MAX_KEY_SET_CHARS) return undefined;
      const keys = await importKeySet(JSON.parse(text) as unknown);
      return keys.size === 0 ? undefined : keys;
    } catch {
      return undefined;
    }
  }
}

// ---------------------------------------------------------------------------------------------------------------
// The verifier
// ---------------------------------------------------------------------------------------------------------------

/** What the verifier needs. The values come from `ACCESS_TEAM_DOMAIN`, `ACCESS_AUD` and `ACCESS_CERTS_URL`. */
export interface AccessConfig {
  /** The host `<team>.cloudflareaccess.com`, without scheme or slash. */
  readonly teamDomain: string;
  /** The AUD tag of the Access application. */
  readonly aud: string;
  /** The host people open in the browser, for example `canvas.patwie.com`. Used in the texts and the Origin check. */
  readonly appHostname: string;
  /** Overrides `https://<teamDomain>/cdn-cgi/access/certs`, for tests and local runs. */
  readonly certsUrl?: string | undefined;
  /** The Rust side. */
  readonly core: AccessCore;
  /** Replaces the global `fetch` (tests). */
  readonly fetch?: typeof fetch | undefined;
  /** Receives the reason word of each refusal. Defaults to `console.warn`. Never receives a token or an email. */
  readonly log?: ((reason: string) => void) | undefined;
}

const JWT_HEADER = "cf-access-jwt-assertion";
const AUTH_COOKIE = "CF_Authorization";

function cookieValue(cookieHeader: string | null, name: string): string | undefined {
  if (cookieHeader === null) return undefined;
  for (const pair of cookieHeader.split(";")) {
    const equals = pair.indexOf("=");
    if (equals >= 0 && pair.slice(0, equals).trim() === name) return pair.slice(equals + 1).trim();
  }
  return undefined;
}

/** True when the `Origin` header names the application's own host. `null` and unparsable values are not ours. */
function isOwnOrigin(origin: string, appHostname: string): boolean {
  try {
    return new URL(origin).host === appHostname;
  } catch {
    return false;
  }
}

function defaultLog(reason: string): void {
  console.warn(`access refused: ${reason}`);
}

type TokenSource = { readonly token: string } | { readonly refused: "missing_token" | "foreign_origin" };

/**
 * The assertion header, else (for a request from our own page, or one with no Origin) the `CF_Authorization` cookie.
 * Without the header, a foreign `Origin` is refused before any cookie is read or parsed.
 */
function tokenOf(request: Request, appHostname: string): TokenSource {
  const header = request.headers.get(JWT_HEADER)?.trim();
  if (header) return { token: header };
  const origin = request.headers.get("origin");
  if (origin !== null && !isOwnOrigin(origin, appHostname)) return { refused: "foreign_origin" };
  const cookie = cookieValue(request.headers.get("cookie"), AUTH_COOKIE);
  return cookie ? { token: cookie } : { refused: "missing_token" };
}

/** Checks the Access assertion of a request: signature through Web Crypto, everything else through the wasm core. */
export class AccessVerifier {
  private readonly keys: KeyStore;
  private readonly log: (reason: string) => void;

  constructor(private readonly config: AccessConfig) {
    const certsUrl = config.certsUrl ?? `https://${config.teamDomain}/cdn-cgi/access/certs`;
    this.keys = new KeyStore(certsUrl, config.fetch ?? fetch);
    this.log = config.log ?? defaultLog;
  }

  /** Decides one request at `nowMs` (milliseconds since the epoch). A refusal is logged by its reason word only. */
  async verify(request: Request, nowMs: number): Promise<AccessOutcome> {
    let outcome: AccessOutcome;
    try {
      outcome = await this.decide(request, nowMs);
    } catch {
      outcome = { ok: false, refusal: INTERNAL_REFUSAL };
    }
    if (!outcome.ok) this.log(outcome.refusal.reason);
    return outcome;
  }

  private deny(reason: string): AccessDenied {
    return { ok: false, refusal: this.config.core.refusal(reason, this.config.appHostname) };
  }

  private async decide(request: Request, nowMs: number): Promise<AccessOutcome> {
    const { core, teamDomain, aud, appHostname } = this.config;
    if (!teamDomain || !aud || !appHostname) return this.deny("not_configured");

    const source = tokenOf(request, appHostname);
    if ("refused" in source) return this.deny(source.refused);
    const { token } = source;

    const parsed = core.parse(token);
    if (!parsed.ok) return this.deny(parsed.reason);

    const lookup = await this.keys.find(parsed.kid, nowMs);
    if ("missing" in lookup) return this.deny(lookup.missing);
    if (!(await signatureHolds(lookup.key, parsed.signature, parsed.signingInput))) return this.deny("bad_signature");

    const claims = core.checkClaims(token, nowMs / 1000, teamDomain, aud);
    return claims.ok ? { ok: true, identity: claims.identity } : this.deny(claims.reason);
  }
}

async function signatureHolds(key: CryptoKey, signature: Uint8Array, signedBytes: Uint8Array): Promise<boolean> {
  try {
    // `new Uint8Array(...)` copies into a plain ArrayBuffer, which is the type Web Crypto asks for.
    return await crypto.subtle.verify(RSA_VERIFY.name, key, new Uint8Array(signature), new Uint8Array(signedBytes));
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------------------------------------------
// The entry point of the Worker
// ---------------------------------------------------------------------------------------------------------------

/** The settings the guard reads. They are the Worker's `vars`; every one may be unset. */
export interface AccessEnv {
  /** `true` or `"true"` turns the check on. */
  readonly ACCESS_REQUIRED?: string | boolean;
  readonly ACCESS_TEAM_DOMAIN?: string;
  readonly ACCESS_AUD?: string;
  /** Test and local override of the certs URL. */
  readonly ACCESS_CERTS_URL?: string;
  /** The host people open; defaults to the host of the request. */
  readonly ACCESS_APP_HOSTNAME?: string;
}

/** What the guard needs besides the settings. */
export interface GuardOptions {
  readonly core: AccessCore;
  readonly fetch?: typeof fetch | undefined;
  readonly log?: ((reason: string) => void) | undefined;
}

const verifiers = new Map<string, AccessVerifier>();

/** Forgets every cached verifier and its key set. For tests. */
export function clearAccessCache(): void {
  verifiers.clear();
}

function verifierFor(config: AccessConfig): AccessVerifier {
  const id = JSON.stringify([config.teamDomain, config.aud, config.appHostname, config.certsUrl ?? null]);
  let verifier = verifiers.get(id);
  if (verifier === undefined) {
    verifier = new AccessVerifier(config);
    verifiers.set(id, verifier);
  }
  return verifier;
}

/**
 * The one call the Worker makes for `/r/*` and `/api/*`. It returns a `401` {@link Response} to send back, or the
 * {@link Request} to forward to the Durable Object or R2: the same request with every Access header and cookie
 * removed. With `ACCESS_REQUIRED` off it checks nothing and still strips. With it on and the team domain or the AUD
 * tag missing it refuses everything.
 *
 * The key sets are cached per isolate, so the first request after a start fetches them and later ones do not.
 */
export async function guard(request: Request, env: AccessEnv, nowMs: number, options: GuardOptions): Promise<Response | Request> {
  if (env.ACCESS_REQUIRED !== true && env.ACCESS_REQUIRED !== "true") return stripIdentity(request);
  const verifier = verifierFor({
    teamDomain: env.ACCESS_TEAM_DOMAIN ?? "",
    aud: env.ACCESS_AUD ?? "",
    appHostname: env.ACCESS_APP_HOSTNAME ?? new URL(request.url).host,
    certsUrl: env.ACCESS_CERTS_URL || undefined,
    core: options.core,
    fetch: options.fetch,
    log: options.log,
  });
  const outcome = await verifier.verify(request, nowMs);
  return outcome.ok ? stripIdentity(request) : denied(outcome);
}
