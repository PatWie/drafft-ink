# Deploying to Cloudflare

One Cloudflare Worker serves the web app (static files) and the end-to-end encrypted collaboration relay (`/r/<room>/ws`
and `/r/<room>/f/...`, one Durable Object per room). The design is in
[design/collab-e2e.md](design/collab-e2e.md) (sections 14 and 19). The Worker is in `apps/drafftink-worker`, and its
README explains the code and the variables.

There are two environments, on two Cloudflare accounts, so that a busy public demo cannot use up the daily quota of the
private one (section 19.11):

| Environment | Domain | Rooms | Pictures | Login |
|---|---|---|---|---|
| `demo` | `drafft.ink` | deleted 48 hours after the last visit | in the room's database | none |
| `private` | your own, e.g. `canvas.patwie.com` | never expire | R2 bucket | Cloudflare Access (Google) |

Until the Access check of the Worker is built, a private deploy answers every request with a refusal. That is on
purpose, so it cannot go live unprotected.

## One-time setup

1. Install Node.js 22.12 or newer (wrangler 4 refuses older versions), Rust with the `wasm32-unknown-unknown` target, and
   `wasm-bindgen-cli` 0.2.130 (`cargo install wasm-bindgen-cli --version 0.2.130 --locked`).
2. Copy `.env.example` to `.env`. It is git-ignored and the only place for settings and secrets. Every value has a
   comment that says where to find it.
3. **Demo:** put the demo account's id in `DEMO_CLOUDFLARE_ACCOUNT_ID`. The zone of `DEMO_DOMAIN` must be active on that
   account. Wrangler creates the DNS record on the first deploy.
4. **Private:** the manual steps (the zone, the R2 bucket, Zero Trust, the Google sign-in, the Access application,
   spending caps) are in [design/collab-e2e.md](design/collab-e2e.md) section 19.3, and the checklist is in
   [roadmap/collab.md](roadmap/collab.md). They give you `PRIVATE_DOMAIN`, `PRIVATE_ACCESS_TEAM_DOMAIN` and
   `PRIVATE_ACCESS_AUD`.
5. Log in with `make cf-login` (one login at a time), or create an API token per account and put it in
   `DEMO_CLOUDFLARE_API_TOKEN` or `PRIVATE_CLOUDFLARE_API_TOKEN`. `.env.example` lists the permissions (section 19.9).
   Do the very first deploy of each environment with the login, then use tokens.

A `CLOUDFLARE_*` variable from your shell is never used for a deploy. Each environment's account id and token reach
wrangler only during its own deploy.

## Commands

| Command | What it does |
|---|---|
| `make help` | Lists the targets. |
| `make web` | Builds the web app into `dist/` (`scripts/build-web.sh --no-precompress`; Cloudflare compresses by itself). |
| `make cf-dev` | Runs the web app and the relay locally with `wrangler dev` on http://127.0.0.1:8787. Demo settings; `CF_ENV=private` for the private ones, `CF_PORT=N` for another port. Builds `dist/` and the Worker's wasm if they are missing. Uses no Cloudflare login and sends nothing. |
| `make cf-config-demo`, `make cf-config-private` | Writes `target/cloudflare/<env>/wrangler.toml` from `.env`. |
| `make cf-check-demo`, `make cf-check-private` | Builds what is missing, writes the config, and runs `wrangler deploy --dry-run`. No login, and an empty `.env` is enough. |
| `make cf-deploy-demo`, `make cf-deploy-private` | Builds web and wasm, writes the config, and deploys that environment. Stops at once, before any build, and names the `.env` variable when a required one is empty. |
| `make cf-login` | Logs in to Cloudflare in the browser. |
| `make cf-whoami` | Shows the account that a deploy of `CF_ENV` (`demo` by default) uses. |

A deploy needs `DEMO_CLOUDFLARE_ACCOUNT_ID` and `DEMO_DOMAIN`, or `PRIVATE_CLOUDFLARE_ACCOUNT_ID`, `PRIVATE_DOMAIN`,
`PRIVATE_ACCESS_TEAM_DOMAIN` and `PRIVATE_ACCESS_AUD`. The API token is optional (without it wrangler uses the login).
A deploy disconnects the open WebSockets of that environment, so deploy on purpose.

## The config

`deploy/cloudflare/wrangler.toml.in` is the one template. `apps/drafftink-worker/scripts/render-config.sh <demo|private>
<file>` fills it in from the variables of `.env` (with safe local defaults for the ones you leave out) and writes the file
that wrangler reads. The Makefile writes it to `target/cloudflare/<env>/wrangler.toml`. Edit the template, not the
generated file. Each environment is its own Worker with its own Durable Object storage. `workers_dev` and preview
addresses are off, so that nothing reaches the app around Cloudflare Access.

`make cf-check-private` works without the Access values: wrangler only copies them into the bundle.

The transport parity suite runs this Worker under `wrangler dev`: see [testing.md](testing.md).

## Notes

- Each deploy uploads only the files that changed.
- The first real-account checks that are still open (daily-limit texts, the 48 hour alarm, Access on WebSockets) are listed
  in section 14.5 and 19.12 of the design.
