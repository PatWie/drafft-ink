import type { Env } from "./env";
import { isOn } from "./env";
import { failure } from "./http";

/**
 * Where the Cloudflare Access check of the private environment plugs in (docs/design/collab-e2e.md, section 19.5).
 * It runs before every route. It answers a refusal, or `null` to let the request through.
 *
 * The check itself (the JWT of the `Cf-Access-Jwt-Assertion` header against the team's keys) is built apart from
 * this file. Until it is wired in here, an environment that asks for Access (`ACCESS_REQUIRED`) refuses every
 * request, so a deploy cannot go live unprotected by mistake.
 */
export function checkAccess(_request: Request, env: Env): Promise<Response | null> {
  if (isOn(env.ACCESS_REQUIRED)) {
    return Promise.resolve(
      failure(503, "access_not_built", "This server requires a Cloudflare Access login, which this build cannot check yet."),
    );
  }
  return Promise.resolve(null);
}
