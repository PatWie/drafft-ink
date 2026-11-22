/** Small JSON replies. The bodies say what happened in plain words, and `error` is a stable word for programs. */

const JSON_HEADERS = { "content-type": "application/json", "cache-control": "no-store" } as const;

/** A JSON reply. */
export function json(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), { status, headers: { ...JSON_HEADERS, ...headers } });
}

/** An error reply: `{"error": code, "message": text}`, the shape of the `drafftink-headless` server. */
export function failure(status: number, error: string, message: string, headers: Record<string, string> = {}): Response {
  return json(status, { error, message }, headers);
}

/** The reply of a Worker or room that cannot serve because the host's daily limit is used up. */
export function dailyLimit(retryAfterSeconds: number): Response {
  const seconds = Math.max(1, Math.ceil(retryAfterSeconds));
  return json(503, { error: "daily_limit", retry_after_s: seconds }, { "retry-after": String(seconds) });
}

/** The header with which a room tells the Worker that a store call hit the daily limit (value: seconds to reset). */
export const DAILY_LIMIT_HEADER = "x-drafft-daily-limit";
