# Run the web app and the relay Worker on Cloudflare. Settings come from .env (copy .env.example). Run `make help`.
# Guide: docs/deploy-cloudflare.md.
#
# Needs: Node.js 22.12+ (for wrangler 4), and the build tools of scripts/build-web.sh and of the Worker
# (Rust with the wasm32 target, wasm-bindgen-cli 0.2.130, binaryen's wasm-opt).
#
# The two environments live on two Cloudflare accounts (docs/design/collab-e2e.md, 19.11): demo (drafft.ink) and
# private (your own domain, behind Cloudflare Access). Each has its own account id and API token in .env.
# A CLOUDFLARE_* variable from your shell is never used for a deploy.

-include .env

# The settings that the config template reads (apps/drafftink-worker/scripts/render-config.sh). Only the ones that .env
# sets are exported, so that the script keeps its own defaults for the others. Account ids and tokens are not here:
# they are handed to wrangler by the deploy and whoami recipes only.
CF_CONFIG_VARS := $(foreach env,DEMO PRIVATE,$(foreach name,WORKER_NAME DOMAIN ROOM_TTL_SECS ROOM_LEAVE_GRACE_SECS \
	BLOB_STORE ACCESS_REQUIRED ACCESS_TEAM_DOMAIN ACCESS_AUD ALLOWED_ORIGINS MAX_ROOM_STATE_BYTES MAX_IMAGE_BYTES \
	MAX_ROOM_IMAGE_BYTES MAX_ROOM_IMAGES ALLOW_DELETE_ROOM,$(env)_$(name))) PRIVATE_R2_BUCKET
export $(foreach var,$(CF_CONFIG_VARS),$(if $(filter undefined,$(origin $(var))),,$(var)))

WRANGLER ?= npx wrangler
CF_ENV   ?= demo
CF_PORT  ?= 8787

WORKER_DIR := $(CURDIR)/apps/drafftink-worker
CF_DIR     := $(CURDIR)/target/cloudflare

.DEFAULT_GOAL := help
.PHONY: help web cf-login cf-whoami cf-dev cf-config-demo cf-config-private cf-check-demo cf-check-private \
	cf-deploy-demo cf-deploy-private cf-run-deploy

# ---- helpers ----------------------------------------------------------------------------------------------------

# The environment that CF_ENV names, in capitals (the prefix of its settings in .env).
cf_prefix = $(if $(filter private,$(1)),PRIVATE,DEMO)
CF_PREFIX = $(call cf_prefix,$(CF_ENV))

ifeq ($(filter $(CF_ENV),demo private),)
$(error CF_ENV is "$(CF_ENV)". It must be demo or private)
endif

# Stops with a plain message unless every named .env variable has a value. $(1) = environment, $(2) = variable names.
define cf_require
@missing='$(strip $(foreach var,$(2),$(if $(strip $($(var))),,$(var))))'; \
if [ -n "$$missing" ]; then \
	echo "Cannot deploy to $(1). Set in .env (copy .env.example): $$missing. See docs/deploy-cloudflare.md."; \
	exit 1; \
fi
endef

# Installs wrangler (the version pinned in package.json) when node_modules is missing.
cf_npm = cd $(WORKER_DIR) && { [ -d node_modules ] || npm ci; }
# Builds the web app into dist/: always, or only when dist/index.html is missing.
cf_web_always = $(MAKE) --no-print-directory web
cf_web_missing = [ -f dist/index.html ] || $(MAKE) --no-print-directory web
# Builds the Rust core of the Worker into apps/drafftink-worker/pkg: always, or only when it is missing.
cf_wasm_always = $(cf_npm) && npm run build:wasm
cf_wasm_missing = $(cf_npm) && { [ -f pkg/drafftink_worker_bg.wasm ] || npm run build:wasm; }
# Writes the wrangler config of an environment. $(1) = demo or private.
cf_render = $(WORKER_DIR)/scripts/render-config.sh $(1) $(CF_DIR)/$(1)/wrangler.toml

# ---- targets ----------------------------------------------------------------------------------------------------

help: ## List the targets
	@grep -hE '^[a-zA-Z0-9_-]+:.*## ' $(firstword $(MAKEFILE_LIST)) | awk -F':.*## ' '{printf "  %-20s %s\n", $$1, $$2}'

web: ## Build the web app into dist/ (no .br/.gz files: Cloudflare compresses by itself)
	scripts/build-web.sh --no-precompress

cf-login: ## Log in to Cloudflare in the browser (not needed with an API token in .env)
	@$(cf_npm)
	cd $(WORKER_DIR) && $(WRANGLER) login

cf-whoami: export CLOUDFLARE_ACCOUNT_ID = $($(CF_PREFIX)_CLOUDFLARE_ACCOUNT_ID)
cf-whoami: export CLOUDFLARE_API_TOKEN = $($(CF_PREFIX)_CLOUDFLARE_API_TOKEN)
cf-whoami: ## Show which Cloudflare account a deploy of CF_ENV (demo or private) uses
	@$(cf_npm)
	@[ -n "$$CLOUDFLARE_API_TOKEN" ] || unset CLOUDFLARE_API_TOKEN; unset CLOUDFLARE_API_KEY CLOUDFLARE_EMAIL; \
	[ -n "$$CLOUDFLARE_ACCOUNT_ID" ] || unset CLOUDFLARE_ACCOUNT_ID; \
	cd $(WORKER_DIR) && $(WRANGLER) whoami

# LOCAL DEV ONLY: CF_ENV=private requires Cloudflare Access, and a local run has none in front of it, so the Worker would
# refuse every request. cf-dev therefore passes `--var ACCESS_REQUIRED:false` for the private environment. That switch
# must never reach a deploy: no cf-check or cf-deploy recipe may contain it (`cargo nextest run -p xtask` checks).
CF_DEV_VARS = $(if $(filter private,$(CF_ENV)),-- --var ACCESS_REQUIRED:false)

cf-dev: ## Run web app and relay locally with wrangler dev (CF_ENV=demo|private, CF_PORT=8787; no login; private runs WITHOUT Access)
	@$(cf_web_missing)
	@$(cf_wasm_missing)
	@$(if $(CF_DEV_VARS),echo "cf-dev: private environment WITHOUT Cloudflare Access (ACCESS_REQUIRED:false); local use only.")
	WRANGLER='$(WRANGLER)' $(WORKER_DIR)/scripts/dev.sh --env $(CF_ENV) --port $(CF_PORT) --no-build $(CF_DEV_VARS)

cf-config-demo: ## Write target/cloudflare/demo/wrangler.toml from .env
	@$(call cf_render,demo)

cf-config-private: ## Write target/cloudflare/private/wrangler.toml from .env
	@$(call cf_render,private)

# The checks need no login and no .env. The Access values of the private environment are not needed either: wrangler
# only copies them into the bundle (the Worker refuses every request while ACCESS_REQUIRED is true and they are empty).
cf-check-demo: ## Build, then check the demo upload without sending anything (no login)
	@$(cf_web_missing)
	@$(cf_wasm_missing)
	@$(call cf_render,demo)
	cd $(WORKER_DIR) && $(WRANGLER) deploy --dry-run --env demo --config $(CF_DIR)/demo/wrangler.toml --outdir $(CF_DIR)/demo/out

cf-check-private: ## Build, then check the private upload without sending anything (no login)
	@$(cf_web_missing)
	@$(cf_wasm_missing)
	@$(call cf_render,private)
	cd $(WORKER_DIR) && $(WRANGLER) deploy --dry-run --env private --config $(CF_DIR)/private/wrangler.toml --outdir $(CF_DIR)/private/out

# A deploy refuses to start (before any build) when a required value of .env is empty. The API token is optional:
# without it wrangler uses the login of `make cf-login`.
cf-deploy-demo: ## Build and deploy the demo (DEMO_* in .env; its own Cloudflare account)
	$(call cf_require,demo,DEMO_CLOUDFLARE_ACCOUNT_ID DEMO_DOMAIN)
	@$(cf_web_always)
	@$(cf_wasm_always)
	@$(call cf_render,demo)
	@$(MAKE) --no-print-directory CF_ENV=demo cf-run-deploy

cf-deploy-private: ## Build and deploy the private instance (PRIVATE_* in .env; its own Cloudflare account)
	$(call cf_require,private,PRIVATE_CLOUDFLARE_ACCOUNT_ID PRIVATE_DOMAIN PRIVATE_ACCESS_TEAM_DOMAIN PRIVATE_ACCESS_AUD)
	@case "$(PRIVATE_ACCESS_REQUIRED)" in ""|true|1) ;; *) \
		echo "Cannot deploy to private: PRIVATE_ACCESS_REQUIRED is \"$(PRIVATE_ACCESS_REQUIRED)\" in .env. The private instance is always deployed with Cloudflare Access required."; exit 1;; esac
	@$(cf_web_always)
	@$(cf_wasm_always)
	@$(call cf_render,private)
	@$(MAKE) --no-print-directory CF_ENV=private cf-run-deploy

# The only place where the account id and the token of an environment reach wrangler (as its own CLOUDFLARE_* variables,
# for this one command). Not for direct use: no description, so `make help` does not list it.
cf-run-deploy: export CLOUDFLARE_ACCOUNT_ID = $($(CF_PREFIX)_CLOUDFLARE_ACCOUNT_ID)
cf-run-deploy: export CLOUDFLARE_API_TOKEN = $($(CF_PREFIX)_CLOUDFLARE_API_TOKEN)
cf-run-deploy:
	@[ -n "$$CLOUDFLARE_API_TOKEN" ] || unset CLOUDFLARE_API_TOKEN; unset CLOUDFLARE_API_KEY CLOUDFLARE_EMAIL; \
	cd $(WORKER_DIR) && $(WRANGLER) deploy --env $(CF_ENV) --config $(CF_DIR)/$(CF_ENV)/wrangler.toml
