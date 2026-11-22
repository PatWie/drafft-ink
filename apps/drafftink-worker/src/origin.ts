/**
 * The `Origin` check of a WebSocket upgrade. Browsers always send the header, so a page of another site cannot
 * open a socket to this relay with the visitor's cookies; native clients send none and pass. With no allowed
 * list, only the origin of the request itself is allowed.
 */
export function originAllowed(origin: string | null, requestUrl: URL, allowed: string): boolean {
  if (origin === null) return true;
  const list = allowed
    .split(/[\s,]+/)
    .map((entry) => entry.trim().replace(/\/+$/, "").toLowerCase())
    .filter((entry) => entry !== "");
  const candidate = origin.trim().replace(/\/+$/, "").toLowerCase();
  if (list.length === 0) return candidate === requestUrl.origin.toLowerCase();
  return list.includes(candidate);
}
