import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["js/test/node/**/*.test.ts"],
    root: new URL("..", import.meta.url).pathname,
    testTimeout: 120_000,
    pool: "forks",
  },
});
