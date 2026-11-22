// The entry that `scripts/private-smoke.mjs` runs under `wrangler dev`: the real Worker and Room, plus one route that
// is not in the product, `/api/__seen/<room>`, which lists the header names the room's object received. It is how the
// script proves, in the real runtime, that the Access headers are gone before the object. Never deployed.
import worker, { Room as ProductionRoom } from "../src/index";

export default {
  async fetch(request: Request, env: Cloudflare.Env): Promise<Response> {
    const seen = /^\/api\/__seen\/([^/]+)$/.exec(new URL(request.url).pathname);
    if (seen?.[1] !== undefined) return env.ROOM.get(env.ROOM.idFromName(seen[1])).fetch(new Request("https://room.invalid/__seen"));
    return worker.fetch(request, env as never);
  },
} satisfies ExportedHandler<Cloudflare.Env>;

export class Room extends ProductionRoom {
  /** The header names (and the cookie header) of each request that reached this object. */
  readonly #seen: { path: string; names: string[]; cookie: string | null }[] = [];

  override async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/__seen") return Response.json(this.#seen);
    this.#seen.push({ path: url.pathname, names: [...request.headers.keys()], cookie: request.headers.get("cookie") });
    return super.fetch(request);
  }
}
