#!/usr/bin/env node
// The private environment in the real local runtime: `wrangler dev --env private` with Access required, R2 for the
// pictures, and a throwaway key set that this script serves over HTTP and signs tokens with. Nothing is deployed and
// no Cloudflare login is used.
//
//   node scripts/private-smoke.mjs [--port 8793]        (from apps/drafftink-worker; needs `npm run build:wasm` once)
//
// It shows: no assertion gives 401; a person's token and a service token (common_name) both work for a WebSocket
// upgrade, an append, a picture upload and its download; a bad signature and an expired token give 401; the object
// receives no `cf-access-*` header and no `CF_Authorization` cookie; and DeleteRoom removes the pictures from R2.
import { spawn } from "node:child_process";
import { createSign, generateKeyPairSync } from "node:crypto";
import { mkdtempSync, readFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";

const app = join(dirname(fileURLToPath(import.meta.url)), "..");
const portAt = process.argv.indexOf("--port");
const port = portAt >= 0 ? Number(process.argv[portAt + 1]) : 8793;
const base = `http://127.0.0.1:${port}`;
const fixtures = JSON.parse(readFileSync(join(app, "test/fixtures/frames.json"), "utf8"));
const hex = (text) => Buffer.from(text, "hex");
const TEAM = "acme.cloudflareaccess.com";
const AUD = "smoke-aud-tag";
const KID = "smoke-key-1";

let failures = 0;
const check = (ok, what) => {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}`);
  if (!ok) failures += 1;
};

// ---- a key set and tokens ----------------------------------------------------------------------------------------
const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
const jwks = { keys: [{ ...publicKey.export({ format: "jwk" }), kid: KID, alg: "RS256", use: "sig" }] };
const b64 = (value) => Buffer.from(typeof value === "string" ? value : JSON.stringify(value)).toString("base64url");
function mint(claims, { tamper = false } = {}) {
  const now = Math.floor(Date.now() / 1000);
  const input = `${b64({ alg: "RS256", kid: KID, typ: "JWT" })}.${b64({ iss: `https://${TEAM}`, aud: [AUD], exp: now + 600, nbf: now - 60, ...claims })}`;
  const signature = createSign("RSA-SHA256").update(input).sign(privateKey).toString("base64url");
  return `${input}.${tamper ? `${signature.slice(0, -4)}AAAA` : signature}`;
}
const person = () => mint({ email: "alice@example.com" });
const machine = () => mint({ common_name: "0123456789abcdef.access" });

const certs = createServer((request, response) => {
  response.setHeader("content-type", "application/json");
  response.end(request.url === "/certs" ? JSON.stringify(jwks) : "{}");
});
await new Promise((resolve) => certs.listen(0, "127.0.0.1", resolve));
const certsUrl = `http://127.0.0.1:${certs.address().port}/certs`;

// ---- wrangler dev ------------------------------------------------------------------------------------------------
const config = join(app, ".wrangler", "smoke", "wrangler.toml");
await new Promise((resolve, reject) => {
  const render = spawn("bash", [join(app, "scripts/render-config.sh"), "private", config], {
    stdio: "inherit",
    env: {
      ...process.env,
      MAIN: join(app, "test/smoke-worker.ts"),
      PRIVATE_ACCESS_REQUIRED: "true",
      PRIVATE_ACCESS_TEAM_DOMAIN: TEAM,
      PRIVATE_ACCESS_AUD: AUD,
      PRIVATE_DOMAIN: "canvas.test",
      PRIVATE_ACCESS_APP_HOSTNAME: "canvas.test",
      PRIVATE_ACCESS_CERTS_URL: certsUrl,
    },
  });
  render.on("exit", (code) => (code === 0 ? resolve() : reject(new Error(`render-config failed: ${code}`))));
});
const state = mkdtempSync(join(tmpdir(), "drafftink-private-smoke-"));
const wrangler = spawn(join(app, "node_modules/.bin/wrangler"), ["dev", "--env", "private", "--config", config, "--ip", "127.0.0.1", "--port", String(port), "--persist-to", state], {
  cwd: app,
  stdio: ["ignore", "pipe", "pipe"],
});
let log = "";
wrangler.stdout.on("data", (d) => (log += d));
wrangler.stderr.on("data", (d) => (log += d));
const stop = () => {
  wrangler.kill("SIGTERM");
  certs.close();
};
process.on("exit", stop);
for (let i = 0; i < 150 && !log.includes("Ready on"); i++) await new Promise((r) => setTimeout(r, 200));
if (!log.includes("Ready on")) {
  console.error(log);
  throw new Error("wrangler dev did not start");
}
console.log(`wrangler dev --env private is up on ${base}, state in ${state}`);

// ---- helpers -----------------------------------------------------------------------------------------------------
const room = fixtures.room.id;
const member = `Drafft-Member ${fixtures.room.token.base64urlNoPad}`;
const image = hex(fixtures.image.sealedHex);
const name = fixtures.image.name;
const call = (path, init = {}, token) =>
  fetch(base + path, { ...init, headers: { ...(token ? { "cf-access-jwt-assertion": token } : {}), ...init.headers } });

/** Opens a socket; resolves with the socket and its frames, or with the HTTP status of a refused upgrade. */
function open(token, extra = {}) {
  return new Promise((resolve) => {
    const ws = new WebSocket(`ws://127.0.0.1:${port}/r/${room}/ws`, { headers: { ...(token ? { "cf-access-jwt-assertion": token } : {}), ...extra } });
    const frames = [];
    ws.on("message", (data, isBinary) => frames.push(isBinary ? Buffer.from(data) : String(data)));
    ws.on("open", () => resolve({ ws, frames }));
    ws.on("unexpected-response", (_request, response) => resolve({ status: response.statusCode }));
    ws.on("error", () => {});
  });
}
const until = async (condition, what) => {
  for (let i = 0; i < 100 && !condition(); i++) await new Promise((r) => setTimeout(r, 50));
  if (!condition()) throw new Error(`timed out waiting for ${what}`);
};
const hasTag = (frames, tag) => frames.some((f) => Buffer.isBuffer(f) && f[0] === tag);

try {
  // 1. No assertion: refused, and the 401 is the JSON of the design.
  for (const [method, path] of [["GET", "/api/status"], ["GET", `/r/${room}/f`], ["PUT", `/r/${room}/f/${name}`]]) {
    const response = await call(path, { method, headers: { authorization: member }, ...(method === "PUT" ? { body: image } : {}) });
    const body = await response.json().catch(() => ({}));
    check(response.status === 401 && body.error === "access_required", `${method} ${path} without a token: 401 access_required`);
  }
  const refused = await open(undefined);
  check(refused.status === 401, "a WebSocket upgrade without a token: 401");

  // 2. Tokens that are not valid.
  check((await call("/api/status", {}, mint({ email: "a@b.c" }, { tamper: true }))).status === 401, "a token with a broken signature: 401");
  check((await call("/api/status", {}, mint({ email: "a@b.c", exp: Math.floor(Date.now() / 1000) - 5 }))).status === 401, "an expired token: 401");
  check((await call("/api/status", {}, mint({ email: "a@b.c", aud: ["other"] }))).status === 401, "another audience: 401");

  // 3. A person: the upgrade works (the request is rebuilt without the Access headers), a record is stored.
  const extra = { "cf-access-authenticated-user-email": "alice@example.com", cookie: "theme=dark; CF_Authorization=secret-cookie; lang=en" };
  const a = await open(person(), extra);
  check(a.ws !== undefined, "a person's token: the WebSocket upgrade is accepted (101)");
  a.ws.send(hex(fixtures.frames.joinCreateA));
  await until(() => hasTag(a.frames, 0x81), "Joined");
  check(true, "the person joined the room (Joined)");
  a.ws.send(hex(fixtures.frames.appendA1));
  await until(() => hasTag(a.frames, 0x84), "Ack");
  check(true, "an append was stored and acknowledged (Ack)");
  a.ws.send("ping");
  await until(() => a.frames.includes("pong"), "pong");
  check(true, "ping is answered with pong");

  // 4. A service token: a second socket, a picture up and down.
  const b = await open(machine());
  check(b.ws !== undefined, "a service token (common_name): the WebSocket upgrade is accepted");
  b.ws.send(hex(fixtures.frames.joinOpenB));
  await until(() => hasTag(b.frames, 0x81), "Joined of the service token");
  const put = await call(`/r/${room}/f/${name}`, { method: "PUT", headers: { authorization: member, cookie: extra.cookie }, body: image }, machine());
  check(put.status === 201, "a service token uploads a picture (201)");
  const get = await call(`/r/${room}/f/${name}`, { headers: { authorization: member } }, person());
  const got = Buffer.from(await get.arrayBuffer());
  check(get.status === 200 && got.equals(image), "a person downloads the same bytes (from R2, by the Worker)");
  const list = await (await call(`/r/${room}/f`, { headers: { authorization: member } }, person())).json();
  check(list.length === 1 && list[0].name === name, "the list comes from the manifest");

  // 5. What the object saw.
  const seen = await (await fetch(`${base}/api/__seen/${room}`)).json();
  const names = seen.flatMap((entry) => entry.names);
  check(seen.length >= 3, `the object received ${seen.length} requests`);
  check(!names.some((n) => n.startsWith("cf-access-")), "no cf-access-* header reached the object");
  check(seen.every((entry) => entry.cookie === null || !entry.cookie.includes("CF_Authorization")), "no CF_Authorization cookie reached the object");
  check(seen.some((entry) => entry.cookie === "theme=dark; lang=en"), "other cookies were kept");

  // 6. DeleteRoom purges R2.
  a.ws.send(hex(fixtures.frames.deleteRoom));
  await until(() => a.ws.readyState === WebSocket.CLOSED, "the socket to close");
  let gone = false;
  for (let i = 0; i < 60 && !gone; i++) {
    gone = (await call(`/r/${room}/f/${name}`, { headers: { authorization: member } }, person())).status === 404;
    if (!gone) await new Promise((r) => setTimeout(r, 100));
  }
  check(gone, "after DeleteRoom the picture is gone from R2 (404)");
  b.ws.close();
} catch (error) {
  console.error(error);
  failures += 1;
  console.error(log.split("\n").slice(-30).join("\n"));
} finally {
  stop();
}
console.log(failures === 0 ? "\nprivate smoke: all checks passed" : `\nprivate smoke: ${failures} check(s) failed`);
process.exit(failures === 0 ? 0 : 1);
