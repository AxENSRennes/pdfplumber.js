import { configDefaults, defineConfig } from "vitest/config";

const runCycleShards = process.env.PDFPLUMBER_JS_RUN_CYCLE_SHARDS === "1";
const vendorExcludes = [
  "pdfjs/**",
  "pdfminer-six/**",
  "pdfplumber-python/**",
  "mupdf.js/**",
  "test/browser/**"
];
const baseExcludes = [...configDefaults.exclude, ...vendorExcludes];

export default defineConfig({
  test: {
    include: ["test/**/*.test.ts"],
    exclude: runCycleShards ? baseExcludes : [...baseExcludes, "test/parity/cycles/**"]
  }
});
