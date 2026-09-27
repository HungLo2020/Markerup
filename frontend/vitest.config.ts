import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // Several jsdom editors are expensive to initialize concurrently on CI.
    maxWorkers: 2,
    testTimeout: 15_000,
    setupFiles: ["./src/test-cleanup.ts"],
  },
});
