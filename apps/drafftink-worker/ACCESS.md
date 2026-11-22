# The Access JWT check of the Worker

Section 19.5 of `docs/design/collab-e2e.md`: Cloudflare Access is the gate, and the Worker is the second lock. A
misconfigured Access application (deleted, wrong hostname, policy set to "Everyone", `workers.dev` left on) must not
open the instance, so the Worker checks the Access JWT itself on `/r/*` and `/api/*`.

| File | What it is |
|---|---|
| `src/access_jwt.rs` | Pure Rust, no clock, no network, no RSA crate. Parses the token, refuses every algorithm but `RS256`, checks the claims, owns every refusal code and text. Native unit tests, and `wasm-bindgen` functions on `wasm32`. |
| `src/access.ts` | The shell: gets and caches the signing keys, verifies the signature with Web Crypto, builds the 401, strips the identity. Needs only `fetch` and `crypto.subtle`. |
| `test/access*.test.ts` | Plain vitest in Node, against the real wasm module. `access-fixtures.ts` holds the helpers. |
| `vitest.access.config.ts` | A node-environment config for the tests above, until the Worker's own vitest setup takes them over. |

## What is checked, in this order

1. Token source: the `Cf-Access-Jwt-Assertion` header. Without it, the `CF_Authorization` cookie, but only when the
   request has no `Origin` or one whose host is the application's host. A foreign `Origin` without the header is
   refused as `foreign_origin` before the cookie is read.
2. Shape (Rust `parse`): at most 8 KiB, header part at most 1 KiB, three base64url parts, header and claims are JSON of
   the expected types. `alg` must be exactly `RS256` (`none`, `HS256`, `rs256`, anything else: `bad_alg`, decided
   before the signature is looked at and before any key is fetched). `typ`, if present, is `JWT`. `kid` is required.
3. Key: chosen by `kid` from the `keys` array (JWKs, not `public_cert`) of the certs document. Unknown `kid`:
   `unknown_kid`. Key set not available: `keys_unavailable`.
4. Signature: `crypto.subtle.verify` with `RSASSA-PKCS1-v1_5` and SHA-256 over the bytes `header.payload`. Fails:
   `bad_signature`.
5. Claims (Rust `check_claims`, only after the signature held): `iss` is `https://<ACCESS_TEAM_DOMAIN>` (`wrong_iss`),
   `aud` (string or list) contains `ACCESS_AUD` (`wrong_aud`), `exp` is in the future, and is required (`expired`),
   `nbf` if present is not more than 5 seconds ahead (`not_yet_valid`, the allowance is `CLOCK_SKEW_SECS`), and an
   identity exists: `email` (a person) or `common_name` (a service token), else `no_identity`.

The result carries only `"person"` or `"machine"`. The email and the client id are never handed to TypeScript, and
`Debug` of the Rust types prints `<hidden>`. A refusal is logged as its reason word only.

### Responses

`401` with `content-type: application/json` and `cache-control: no-store`, never a redirect:

- no token (`missing_token`, `foreign_origin`): `{"error":"access_required","message":"This server is only open to signed-in, approved users. Open https://<app host> in your browser and sign in with Google. In the desktop app, enter the access token for this server in Preferences."}`
- every other refusal: `{"error":"access_invalid","message":"Your sign-in is not valid for this server or has expired. Reload the page and sign in again."}`

Reason words for the log: `missing_token`, `foreign_origin`, `too_large`, `malformed`, `bad_alg`, `unknown_kid`,
`keys_unavailable`, `bad_signature`, `wrong_iss`, `wrong_aud`, `expired`, `not_yet_valid`, `no_identity`,
`not_configured` (the team domain or the AUD tag is empty: everything is refused), and `internal` (the wasm module
threw: fail closed).

### The key set

Fetched from `https://<team>.cloudflareaccess.com/cdn-cgi/access/certs` (or `ACCESS_CERTS_URL`). Cached one hour
(`KEY_SET_TTL_MS`). An unknown `kid` triggers one refetch, and not more than one a minute
(`UNKNOWN_KID_REFETCH_MS`); then the token is refused. Concurrent requests share one fetch. A failed fetch (network
error, non-200, not JSON, no usable key, more than 3 s) refuses with `keys_unavailable` and is remembered for 5 seconds
(`FETCH_FAILURE_BACKOFF_MS`) only. An expired key set is not trusted when the refresh fails. The key set lives in
module state, so it is per isolate: the first request after a start fetches it (one subrequest, a few milliseconds of
wall time, not CPU).

### Stripping

`stripIdentity` removes every header that starts with `cf-access-` (the assertion, the authenticated email,
`CF-Access-Client-*`, `cf-access-token`) and the cookies `CF_Authorization`, `CF_AppSession` and `CF_Binding`. Other
cookies and headers stay. Method, URL, body and an upgrade request are untouched. The names `CF_AppSession` and
`CF_Binding` are from memory of the Access docs, not from a real response: confirm on the first real run.

## Settings

| Name | Meaning |
|---|---|
| `ACCESS_REQUIRED` | `true` (or the string `"true"`) turns the check on. Off: the guard checks nothing and still strips. |
| `ACCESS_TEAM_DOMAIN` | The host `<team>.cloudflareaccess.com`, no scheme, no slash. |
| `ACCESS_AUD` | The AUD tag of the Access application. |
| `ACCESS_CERTS_URL` | Optional. Replaces the certs URL, for tests and local runs. |
| `ACCESS_APP_HOSTNAME` | Optional, new. The host people open (`canvas.patwie.com`). Used in the 401 text and the `Origin` rule. Defaults to the host of the request URL, which is wrong on a `workers.dev` host: set it in the `private` environment. |

## Wiring (done by the Worker's `src/index.ts`)

```ts
import { guard, coreFromWasm } from "./access";
import * as wasm from "../pkg/drafftink_worker"; // after initSync({ module }) as in docs/design/collab-spike.md, section 5

const accessCore = coreFromWasm(wasm); // exports access_parse, access_check_claims, access_refusal

// in fetch(request, env), for /r/* and /api/* only:
const checked = await guard(request, env, Date.now(), { core: accessCore });
if (checked instanceof Response) return checked; // 401 JSON: the object and R2 are never called
const forward = checked;                          // the request without Access headers and cookies
return env.ROOM.get(id).fetch(forward);
```

`env` may be the Worker's whole `Env`: `guard` reads only the five names above. `clearAccessCache()` forgets the
cached key sets (tests).

## Running the tests

Rust, native: `cargo nextest run -p drafftink-worker access`.

TypeScript, with the real wasm (Node 22):

```bash
cargo build -p drafftink-worker --lib --target wasm32-unknown-unknown --release
wasm-bindgen --target nodejs --out-dir target/access-wasm target/wasm32-unknown-unknown/release/drafftink_worker.wasm
cd apps/drafftink-worker && npx vitest run --config vitest.access.config.ts
```

`ACCESS_WASM_DIR` overrides `target/access-wasm`. Fragment for the Worker's `package.json` (the file belongs to the
Worker's author; versions are the ones the tests ran with):

```json
{
  "scripts": { "test:access": "vitest run --config vitest.access.config.ts" },
  "devDependencies": { "vitest": "^5.1.1", "typescript": "^7.0.2", "@types/node": "^22" }
}
```

If the Worker's own vitest config already includes `test/**/*.test.ts` in a Node-compatible pool, `vitest.access.config.ts`
and the script are not needed. The tests import `../src/access`, use `node:` modules and need the wasm built for Node.

## What the tests cover

Rust (31 tests): valid person and service token; wrong `aud` (string, list, empty, missing); wrong `iss` (other team,
`http`, trailing slash, suffix trick); expired, `exp` equal to now, missing `exp`; `nbf` ahead (beyond and within the
skew); no identity (missing, empty, null); order of the checks; `alg: none`, HS256 signed with the public key,
every other algorithm; header without `kid`, wrong `typ`; truncated, misshapen, oversize tokens; no leak in `Debug`;
codes, reason words and texts; random strings, random base64url parts, random JSON bytes and a byte-by-byte mutation of
a valid token never panic (proptest).

TypeScript (53 tests): the cases above through the whole path with real RSA 2048 signatures; tampered payload; an
attacker key with a known `kid`; unknown `kid` (one refetch, none inside the minute, one after it); rotation (both keys
verify, a new key is picked up); one-hour cache; one fetch for concurrent requests; four ways the fetch fails, recovery
after 5 seconds; missing header is `access_required`, garbage is `access_invalid`; cookie fallback and the `Origin`
rule; stripping (other cookies kept, body kept); the guard never calls the stub object when it refuses; logs contain
reason words and no token, token part, email or client id.

## Not verified

- **Real Access on a WebSocket upgrade** (`docs/design/collab-spike.md`, section 6): whether the browser's cookie and a
  service token's headers on an upgrade come out as a `Cf-Access-Jwt-Assertion` header that this code accepts, what a
  session that ends does to an open socket, and what an Access-blocked request looks like. Nothing here can be tested
  without a real account.
- The real certs document. The tests use the shape described in the docs (`keys` with `kty`, `kid`, `n`, `e`, `alg`,
  `use`, and `public_cert`); a real response is unchecked.
- CPU time on the Free plan. The signature check is one `crypto.subtle.verify`, and the wasm side parses at most 8 KiB;
  both should be well under 10 ms, but only a deployed Worker can say.
- `new Request(request, { headers })` for a WebSocket upgrade in workerd (the Node tests cover the headers, the body and
  the method, not the upgrade).
- The names of the cookies `CF_AppSession` and `CF_Binding`.
