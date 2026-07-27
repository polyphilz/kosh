import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/browser",
  outputDir: "./test-results/playwright",
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:1422",
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  webServer: {
    command: "pnpm dev:test",
    url: "http://127.0.0.1:1422",
    reuseExistingServer: false,
  },
});
