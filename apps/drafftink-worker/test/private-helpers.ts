import { env } from "cloudflare:workers";
import { abortAllDurableObjects, runInDurableObject } from "cloudflare:test";

import { Client, ROOM, request, roomStub } from "./helpers";
import { probe } from "./worker";

export const PRIVATE_ORIGIN = "https://canvas.test";
export const TEAM = "acme.cloudflareaccess.com";
export const AUD = "aud-tag-1234";

const RSA = { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" } as const;

function b64url(bytes: Uint8Array | string): string {
  const raw = typeof bytes === "string" ? new TextEncoder().encode(bytes) : bytes;
  return btoa(String.fromCharCode(...raw)).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

export type Claims = Record<string, unknown>;

/** A token signed with the test key. `claims` are merged over the defaults of a person. */
export async function mint(claims: Claims = {}, header: Claims = {}): Promise<string> {
  const key = await crypto.subtle.importKey("jwk", JSON.parse(env.TEST_ACCESS_PRIVATE_JWK) as JsonWebKey, RSA, false, ["sign"]);
  const now = Math.floor(Date.now() / 1000);
  const head = { alg: "RS256", kid: env.TEST_ACCESS_KEY_ID, typ: "JWT", ...header };
  const body = { iss: `https://${TEAM}`, aud: [AUD], exp: now + 600, nbf: now - 60, email: "alice@example.com", ...claims };
  const input = `${b64url(JSON.stringify(head))}.${b64url(JSON.stringify(body))}`;
  const signature = await crypto.subtle.sign(RSA.name, key, new TextEncoder().encode(input));
  return `${input}.${b64url(new Uint8Array(signature))}`;
}

/** A person's token. */
export const person = (): Promise<string> => mint();
/** A service token's token: `common_name` and no email. */
export const machine = (): Promise<string> => mint({ email: undefined, common_name: "0123456789abcdef.access" });

export const ASSERTION = "cf-access-jwt-assertion";

/** A request as a signed-in person's browser or a service token's client would send it. */
export async function signedRequest(path: string, init: RequestInit = {}, token?: string): Promise<Response> {
  const headers = new Headers(init.headers);
  headers.set(ASSERTION, token ?? (await person()));
  return request(path, { ...init, headers }, PRIVATE_ORIGIN);
}

/** A WebSocket to a room with a valid assertion. */
export async function signedClient(room: string = ROOM, token?: string): Promise<Client> {
  return Client.open(room, { [ASSERTION]: token ?? (await person()) });
}

/** Everything the bucket holds, as keys. */
export async function r2Keys(): Promise<string[]> {
  const keys: string[] = [];
  let cursor: string | undefined;
  do {
    const page = await env.BLOBS.list(cursor === undefined ? {} : { cursor });
    keys.push(...page.objects.map((o) => o.key));
    cursor = page.truncated ? page.cursor : undefined;
  } while (cursor !== undefined);
  return keys;
}

/** Empties the bucket and the room's storage, and lets the object go. */
export async function resetAll(): Promise<void> {
  probe().clockOffsetMs = 0;
  probe().failR2 = { put: null, list: null, delete: null };
  probe().r2PageSize = null;
  probe().failStorageWith = null;
  for (const key of await r2Keys()) await env.BLOBS.delete(key);
  await runInDurableObject(roomStub(), async (_instance, state) => {
    await state.storage.deleteAlarm();
    await state.storage.deleteAll();
  });
  await abortAllDurableObjects();
}

/** Waits until `condition` holds: the room cleans up after it has closed the sockets, so tests wait for the end. */
export async function waitFor(condition: () => Promise<boolean>, what: string, timeoutMs = 5_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!(await condition())) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
}
