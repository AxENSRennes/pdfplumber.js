import { configDefaults, defineConfig } from "vitest/config";

const runCycleShards = process.env.PDFPLUMBER_JS_RUN_CYCLE_SHARDS === "1";
const vendorExcludes = [
  "pdfjs/**",
  "pdfminer-six/**",
  "pdfplumber-python/**",
  "mupdf.js/**"
];
const baseExcludes = [...configDefaults.exclude, ...vendorExcludes];

export default defineConfig({
  test: {
    exclude: runCycleShards ? baseExcludes : [...baseExcludes, "test/parity/cycles/**"]
  }
});
