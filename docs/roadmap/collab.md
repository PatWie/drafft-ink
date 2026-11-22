# Collaboration: plan and to-do (later)

Status: **phases 1a, 1b, 2, 3 and 3P are built and tested locally; nothing is deployed.** 1a: the `seal` crate and protocol v2 (`docs/protocol.md` section 11). 1b: the relay core `crates/relay`, relay mode in `drafftink-headless serve`, the sealed client in `crates/collab`, the `collab.*` changes and the workspace schema 2 (`docs/design/collab-e2e.md` section 14.2). 2: pictures as encrypted files (14.3). 3: the Worker and Durable Object of the demo, `apps/drafftink-worker` (14.5). 3P: the private environment, the Access check, R2 pictures, `DeleteRoom` and the service token in the native client (14.6, 14.7). Left: phase 4 (the deploy of drafft.ink, Turnstile, the `Registry` object, spending caps) and the steps by hand below. Named versions come first. The full design is [docs/design/collab-e2e.md](../design/collab-e2e.md); this page is the short plan and checklist.

## What we build

1. **End-to-end encrypted rooms.** The room link carries the key after `#` (`drafft.ink/#room=<id>,<key>`). Every change, cursor and image is locked in the browser. A relay only stores and forwards ciphertext.
2. **Public demo on drafft.ink.** One Cloudflare Durable Object per room, on the Workers free plan (paid plan later is only a plan change). Rooms and their images are deleted 48 h after the last visit (`ROOM_TTL`); a new visit restarts the 48 h. Images are stored in the room, at most 4 MiB each (`MAX_IMAGE_BYTES`).
3. **Self-host.** The same relay core runs in `drafftink-headless serve`.
4. **Private instance at `canvas.patwie.com`.** Same app and relay on the user's own Cloudflare account:
   - rooms never expire; images in an R2 bucket (no lifecycle rule);
   - Google sign-in for an email allow-list through Cloudflare Access on the whole subdomain;
   - the Worker re-checks the Access login token itself;
   - the desktop app and agents use an Access service token, entered once in Preferences;
   - `workers.dev` and preview URLs are turned off (they would bypass the login);
   - kept on a different Cloudflare account than the demo, because spending caps and free limits are per account.

## Phases

| Phase | What | Days |
|---|---|---|
| 0 | Trial run on Cloudflare (`wrangler dev` plus a real free account): alarms, CPU limit, quota errors, Access cookie and service-token headers on a WebSocket upgrade, open socket after session expiry | 2 |
| 1 | Encryption layer and protocol v2, relay core (`drafftink-relay`), tested against `drafftink-headless serve` relay mode | 30 |
| 2 | Images as separate encrypted files (`BlobStore`, `BlobTransport`) | 15 |
| 3 | Cloudflare Worker and Durable Object for the demo | 11 |
| 3P | Private instance (R2, Access check, service tokens, `private` environment) | 11.5 |
| 4 | drafft.ink deploy (wrangler, secrets, spending caps) | 3 |

About 73 days in total. Named versions must keep working in encrypted rooms (they are data in the document, so they survive snapshot compaction).

## Done by the user

- [x] Payment card on the Cloudflare account (needed for Zero Trust and R2).
- [x] Wrangler deploys already work on that account from another machine.

## To do by hand (phase 3P, before the first private deploy)

- [ ] `patwie.com` zone on Cloudflare (if not already).
- [ ] R2 bucket `drafftink-private-blobs`, lifecycle rule off.
- [ ] Zero Trust team on the free plan.
- [ ] Google OAuth client, redirect `https://<team>.cloudflareaccess.com/cdn-cgi/access/callback`; add Google as a login method in Zero Trust.
- [ ] Access application for the whole subdomain `canvas.patwie.com`: Emails allow policy, Service Auth policy, session length; copy the AUD tag into the Worker config.
- [ ] Service tokens for the desktop app and agents.
- [ ] Spending limits on Workers and Durable Objects; usage alerts for R2 (R2 and Access cannot be capped).
- [ ] Fill in `.env` (`.env.example` lists every value, per environment) and run `make cf-check-private`, then the first `make cf-deploy-private` yourself (`make cf-login` once). Afterwards request the `workers.dev` URL (it must be dead) and run the smoke checklist of `docs/design/collab-e2e.md` 19.10.
- [ ] Run the real-account checks of `apps/drafftink-worker/README.md` and `docs/design/collab-e2e.md` 14.5 and 14.7 (Access cookie and service token on a WebSocket upgrade, session expiry with an open socket, Free-plan CPU and quota error texts).

## Open questions (answer before phase 1)

1. Room id derived from the key (no stored check)? Proposed: yes.
2. Keep the room key in the local browser save so a reload reconnects? Proposed: yes.
3. Rebuild an expired room only through its link? Proposed: yes.
4. Should `serve` still accept plaintext v1 rooms? Proposed: encrypted by default, v1 behind a flag.
5. Oversized picture: refuse or offer to shrink? Proposed: offer to shrink.
6. 60 s grace after the last person leaves? Proposed: yes.
7. Turnstile on room creation for the demo? Proposed: only if abused.
8. Worker in Rust (workers-rs, beta) or a TypeScript shell around the Rust core? Proposed: TypeScript shell.
