// A plain Node run of the Access tests, for use before the Worker's own vitest setup exists. See ACCESS.md.
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    include: ["test/access*.test.ts"],
    testTimeout: 20_000,
  },
});
