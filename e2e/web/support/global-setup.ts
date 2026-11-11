/** Starts one server for the whole run, checks that the browser has a WebGPU adapter, and tells the workers. */
import { chromium } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { ADAPTER, CHROMIUM_ARGS, REQUIRE_GPU, SERVER_INFO } from "./env.ts";
import { startServer } from "./server.ts";

/** Opens headed Chromium and asks for a WebGPU adapter. Returns the adapter's one-line description, or why there is none. */
async function probeAdapter(origin: string): Promise<{ adapter: string } | { reason: string }> {
  if (!process.env["DISPLAY"]) return { reason: "no display: DISPLAY is unset. Run under Xvfb, or use `cargo xtask e2e`" };
  let browser;
  try {
    browser = await chromium.launch({ headless: false, args: CHROMIUM_ARGS });
  } catch (e) {
    return { reason: `Chromium could not start headed (no display?): ${String(e).split("\n")[0]}` };
  }
  try {
    const page = await browser.newPage();
    // WebGPU exists in secure contexts only: http://127.0.0.1 is one, about:blank is not.
    await page.goto(`${origin}/health`);
    const info = await page.evaluate(async () => {
      const gpu = (navigator as unknown as { gpu?: { requestAdapter(): Promise<{ info?: { vendor?: string; architecture?: string; description?: string } } | null> } }).gpu;
      if (!gpu) return null;
      const adapter = await gpu.requestAdapter();
      if (!adapter) return null;
      const i = adapter.info ?? {};
      return `${i.vendor ?? "?"} ${i.architecture ?? ""} ${i.description ?? ""}`.trim();
    });
    return info === null ? { reason: `Chromium has no WebGPU adapter with the ${ADAPTER} flags` } : { adapter: info };
  } finally {
    await browser.close();
  }
}

export default async function globalSetup(): Promise<() => Promise<void>> {
  const server = await startServer();
  const probe = await probeAdapter(server.origin);
  if ("reason" in probe && REQUIRE_GPU) {
    await server.stop();
    throw new Error(`${probe.reason} (DRAFFTINK_REQUIRE_GPU is set)`);
  }
  if ("reason" in probe) console.log(`SKIPPED: browser suite: ${probe.reason}`);
  else console.log(`WebGPU adapter (${ADAPTER} flags): ${probe.adapter}`);
  mkdirSync(path.dirname(SERVER_INFO), { recursive: true });
  const skip = "reason" in probe ? probe.reason : null;
  writeFileSync(SERVER_INFO, JSON.stringify({ port: server.port, origin: server.origin, altOrigin: server.altOrigin, skip }));
  return () => server.stop();
}
