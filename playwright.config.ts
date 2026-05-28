import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "test/browser",
  fullyParallel: false,
  reporter: "list",
  use: {
    trace: "on-first-retry"
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] }
    },
    {
      name: "firefox",
      use: { ...devices["Desktop Firefox"] }
    },
    {
      name: "webkit",
      use: { ...devices["Desktop Safari"] }
    }
  ]
});
