import { DAILY_LIMIT_HEADER } from "./http";
import type { ImageReply } from "./wasm";

/** The body of a request, or `null` when it is longer than `limit`. Never holds more than `limit + 1` bytes. */
export async function readLimited(request: Request, limit: number): Promise<Uint8Array<ArrayBuffer> | null> {
  const declared = Number(request.headers.get("content-length") ?? "0");
  if (Number.isFinite(declared) && declared > limit) return null;
  if (request.body === null) return new Uint8Array(0);
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > limit) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const body = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return body;
}

/** The HTTP reply for an answer of the core. `head` drops the body but keeps its length. */
export function imageResponse(reply: ImageReply, head: boolean): Response {
  const headers: Record<string, string> = { "content-type": reply.contentType, "cache-control": "no-store" };
  const retry = reply.retryAfterMs;
  if (retry !== undefined) headers["retry-after"] = String(Math.max(1, Math.ceil(retry / 1000)));
  if (reply.code === "daily_limit" && retry !== undefined) headers[DAILY_LIMIT_HEADER] = headers["retry-after"] ?? "1";
  const body = reply.body();
  if (head) {
    headers["content-length"] = String(body.byteLength);
    return new Response(null, { status: reply.status, headers });
  }
  return new Response(body, { status: reply.status, headers });
}
