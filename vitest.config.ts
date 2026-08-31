import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  define: {
    __APP_VERSION__: JSON.stringify(process.env.npm_package_version ?? "unknown"),
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
    clearMocks: true,
    // The jsdom-heavy App suites can exceed Vitest's per-test timeout when a
    // high-core machine starts one worker per file. Two workers are faster in
    // practice and keep local/CI runs deterministic instead of CPU-thrashing.
    maxWorkers: 2,
    coverage: {
      reporter: ["text", "html"],
    },
  },
});
