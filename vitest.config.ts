import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    fileParallelism: false,
    include: ["test/**/*.test.ts"],
    exclude: ["node_modules/**", "dist/**", "pdfjs/**", "pdfminer-six/**", "pdfplumber-python/**", "mupdf.js/**"]
  }
});
