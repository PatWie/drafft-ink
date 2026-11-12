/** Paths and settings shared by the config, the global setup and the support code. */
import { fileURLToPath } from "node:url";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));

/** The repository root. */
export const ROOT = path.resolve(here, "../../..");

/** Where `scripts/build-web.sh` writes the web build. */
export const DIST = process.env["DRAFFTINK_E2E_DIST"] ?? path.join(ROOT, "dist");

/** `drafftink-headless`, whose `serve` is the collaboration and static server. */
export const HEADLESS_BIN =
  process.env["DRAFFTINK_E2E_HEADLESS"] ?? path.join(process.env["CARGO_TARGET_DIR"] ?? path.join(ROOT, "target"), "release/drafftink-headless");

/** Fixture documents of the repository (`tests/fixtures`). */
export const FIXTURES = path.join(ROOT, "tests/fixtures");

/** Where the global setup tells the workers about the server it started. */
export const SERVER_INFO = path.join(ROOT, "e2e/web/results/server.json");

/** Chromium flags that give WebGPU on the NVIDIA GPU under Xvfb (research/web-perf.md, scripts/web_check.py). */
const NVIDIA = [
  "--enable-unsafe-webgpu",
  "--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan",
  "--use-angle=vulkan",
  "--use-vulkan=native",
  "--ignore-gpu-blocklist",
  "--disable-vulkan-surface",
  "--no-sandbox",
  "--disable-gpu-sandbox",
];

/** Chromium flags for the CPU adapter (SwiftShader): slow, but it needs no GPU. */
const SWIFTSHADER = [
  "--enable-unsafe-webgpu",
  "--enable-features=Vulkan",
  "--use-angle=swiftshader",
  "--enable-unsafe-swiftshader",
  "--use-webgpu-adapter=swiftshader",
  "--ignore-gpu-blocklist",
  "--no-sandbox",
];

/** `DRAFFTINK_E2E_ADAPTER=swiftshader` selects the CPU adapter; the default is the NVIDIA GPU. */
export const ADAPTER: "nvidia" | "swiftshader" = process.env["DRAFFTINK_E2E_ADAPTER"] === "swiftshader" ? "swiftshader" : "nvidia";

/** The browser flags for the chosen adapter. */
export const CHROMIUM_ARGS: string[] = ADAPTER === "swiftshader" ? SWIFTSHADER : NVIDIA;

/** `DRAFFTINK_REQUIRE_GPU=1`: a machine that must have a GPU fails instead of skipping. */
export const REQUIRE_GPU: boolean = !["", "0", undefined].includes(process.env["DRAFFTINK_REQUIRE_GPU"]);
