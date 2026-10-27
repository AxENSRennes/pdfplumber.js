// Browser tests (scripts/check-browsers.sh): the shared runtime tests in
// Chromium, Firefox and WebKit, served by js/test/serve.mjs. Needs dist/
// (node scripts/build.mjs) and the js_runtime unit data
// (python -m oracle.unit.js_runtime).
import { defineConfig, devices } from "@playwright/test";

const port = Number(process.env.PDFPLUMBER_TEST_PORT ?? 4173);

export default defineConfig({
  testDir: "test/browser",
  timeout: 180_000,
  fullyParallel: false,
  workers: 1,
  reporter: [["list"]],
  use: { baseURL: `http://127.0.0.1:${port}` },
  webServer: {
    command: `node js/test/serve.mjs ${port}`,
    cwd: new URL("..", import.meta.url).pathname,
    url: `http://127.0.0.1:${port}/js/test/browser/page.html`,
    reuseExistingServer: false,
  },
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
    { name: "firefox", use: { ...devices["Desktop Firefox"] } },
    {
      name: "webkit",
      use: {
        ...devices["Desktop Safari"],
        // the launcher of scripts/webkit-local-libs.sh, on a machine without
        // WebKit's system libraries (see docs/js-runtime.md, "Tests")
        launchOptions: process.env.PDFPLUMBER_WEBKIT ? { executablePath: process.env.PDFPLUMBER_WEBKIT } : {},
      },
    },
  ],
});
