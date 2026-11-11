import { defineConfig } from "@playwright/test";
import { CHROMIUM_ARGS } from "./support/env.ts";

// Headed Chromium: a headless one draws WebGPU canvases blank (research/web-perf.md). `cargo xtask e2e`
// starts the Xvfb display; on a desktop the window just opens.
export default defineConfig({
  testDir: "./specs",
  globalSetup: "./support/global-setup.ts",
  // One worker: the idle checks count frames and must not share the GPU with another page.
  workers: 1,
  fullyParallel: false,
  timeout: 120_000,
  expect: { timeout: 10_000 },
  reporter: [["list"], ["json", { outputFile: "results/report.json" }]],
  outputDir: "results/artifacts",
  use: {
    // Without a display the specs are skipped (see support/global-setup.ts); headless only lets them reach that skip.
    headless: !process.env["DISPLAY"],
    viewport: { width: 1280, height: 800 },
    launchOptions: { args: CHROMIUM_ARGS },
    trace: "off",
    screenshot: "only-on-failure",
  },
});
