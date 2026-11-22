# Deploy the web app to Cloudflare. Settings come from .env (copy .env.example). Run `make help`.
#
# Needs: Node.js 22.12+ (for wrangler 4), and the web build tools of scripts/build-web.sh
# (Rust with the wasm32 target, wasm-bindgen-cli 0.2.130, binaryen's wasm-opt).

-include .env
export CLOUDFLARE_ACCOUNT_ID CLOUDFLARE_API_TOKEN

WRANGLER       ?= npx --yes wrangler@4
CF_WORKER_NAME ?= drafftink-canvas
DIST_DIR       := $(CURDIR)/dist
CF_DIR         := $(CURDIR)/target/cloudflare
CF_CONFIG      := $(CF_DIR)/wrangler.toml

.DEFAULT_GOAL := help
.PHONY: help web cf-login cf-whoami cf-config cf-dev cf-check cf-deploy

help: ## List the targets
	@grep -hE '^[a-z-]+:.*## ' $(firstword $(MAKEFILE_LIST)) | awk -F':.*## ' '{printf "  %-12s %s\n", $$1, $$2}'

web: ## Build the web app into dist/ (no .br/.gz files: Cloudflare compresses by itself)
	scripts/build-web.sh --no-precompress

cf-login: ## Log in to Cloudflare in the browser (not needed with CLOUDFLARE_API_TOKEN)
	$(WRANGLER) login

cf-whoami: ## Show which Cloudflare account wrangler uses
	$(WRANGLER) whoami

cf-config: ## Write target/cloudflare/wrangler.toml from .env
	@test -n "$(CF_DOMAIN)" || { echo "CF_DOMAIN is not set. Copy .env.example to .env and fill it in."; exit 1; }
	@mkdir -p $(CF_DIR)
	@sed -e 's|@CF_WORKER_NAME@|$(CF_WORKER_NAME)|g' -e 's|@CF_DOMAIN@|$(CF_DOMAIN)|g' -e 's|@ASSETS_DIR@|$(DIST_DIR)|g' \
		deploy/cloudflare/wrangler.toml.in > $(CF_CONFIG)
	@echo "wrote $(CF_CONFIG) for $(CF_WORKER_NAME) on $(CF_DOMAIN)"

cf-dev: cf-config ## Serve dist/ locally with the Cloudflare runtime (http://localhost:8787)
	$(WRANGLER) dev --config $(CF_CONFIG)

cf-check: cf-config ## Check the config and files without uploading (no login needed)
	@test -f $(DIST_DIR)/index.html || { echo "dist/ is empty. Run 'make web' first."; exit 1; }
	$(WRANGLER) deploy --config $(CF_CONFIG) --dry-run --outdir $(CF_DIR)/out

cf-deploy: web cf-config ## Build the web app and deploy it to CF_DOMAIN
	@test -n "$(CLOUDFLARE_ACCOUNT_ID)" || { echo "CLOUDFLARE_ACCOUNT_ID is not set in .env."; exit 1; }
	$(WRANGLER) deploy --config $(CF_CONFIG)
