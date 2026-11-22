# Deploying to Cloudflare

The web app can run on your own Cloudflare account under your own domain, e.g. `canvas.patwie.com`. It is the same
app as drafft.ink. Today it is static files only. The collaboration relay will be added to the same Worker later
(see [roadmap/collab.md](roadmap/collab.md)).

## One-time setup

1. Install Node.js 22.12 or newer. Wrangler 4 refuses older versions.
2. Copy `.env.example` to `.env` and fill in `CLOUDFLARE_ACCOUNT_ID` and `CF_DOMAIN`. `.env` is git-ignored.
3. Log in: `make cf-login`, which opens the browser. On a machine without a browser, put a `CLOUDFLARE_API_TOKEN` in
   `.env` instead. The permissions it needs are listed in `.env.example`.
4. The zone of `CF_DOMAIN` (for example `patwie.com`) must be on the same Cloudflare account. Wrangler creates the
   DNS record for the subdomain on the first deploy.

## Commands

| Command | What it does |
|---|---|
| `make help` | Lists the targets. |
| `make web` | Builds the app into `dist/` (`scripts/build-web.sh --no-precompress`; Cloudflare compresses by itself). |
| `make cf-check` | Writes the config and checks the upload without sending anything. No login needed. |
| `make cf-dev` | Serves `dist/` locally with the Cloudflare runtime on http://localhost:8787. |
| `make cf-deploy` | Builds and deploys to `CF_DOMAIN`. |
| `make cf-whoami` | Shows which account wrangler uses. |

The config is generated from `deploy/cloudflare/wrangler.toml.in` into `target/cloudflare/wrangler.toml`. Edit the
template, not the generated file.

## Notes

- The automatic `*.workers.dev` address and preview addresses are turned off (`workers_dev = false`,
  `preview_urls = false`). If you put Cloudflare Access (for example Google sign-in) in front of `CF_DOMAIN`, those
  addresses would otherwise reach the app without the login.
- Google sign-in for an email list: Cloudflare Zero Trust, then Access, then Applications. Add a self-hosted application for
  the whole hostname `CF_DOMAIN` with an "Emails" allow policy. The steps are in
  [design/collab-e2e.md](design/collab-e2e.md) section 19.
- Each deploy uploads only files that changed.
