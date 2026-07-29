import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/browser",
  outputDir: "./test-results/playwright",
  snapshotPathTemplate: "{testDir}/__snapshots__/{arg}{ext}",
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:1422",
    locale: "en-US",
    timezoneId: "UTC",
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium-functional",
      testIgnore: [/visual\.spec\.ts/, /webkit-contract\.spec\.ts/],
      use: { ...devices["Desktop Chrome"] },
    },
    {
      name: "webkit-contract",
      testMatch: /webkit-contract\.spec\.ts/,
      use: { ...devices["Desktop Safari"] },
    },
    {
      name: "chromium-visual",
      testMatch: /visual\.spec\.ts/,
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  webServer: {
    command: "pnpm dev:test",
    url: "http://127.0.0.1:1422",
    reuseExistingServer: false,
  },
});
