import { expect, test as base } from "@playwright/test";

const allowedProtocols = new Set(["about:", "blob:", "data:", "kosh-media:"]);
const allowedOrigins = new Set(["http://127.0.0.1:1422"]);

export const test = base.extend<{ healthyPage: void }>({
  healthyPage: [
    async ({ page }, use) => {
      const failures: string[] = [];

      page.on("console", (message) => {
        if (message.type() === "error") {
          failures.push(`console: ${message.text()}`);
        }
      });
      page.on("pageerror", (error) => {
        failures.push(`page: ${error.message}`);
      });
      page.on("request", (request) => {
        const url = new URL(request.url());
        if (!allowedProtocols.has(url.protocol) && !allowedOrigins.has(url.origin)) {
          failures.push(`external request: ${request.method()} ${url.origin}${url.pathname}`);
        }
      });
      page.on("requestfailed", (request) => {
        failures.push(
          `request failed: ${request.method()} ${request.url()} (${request.failure()?.errorText ?? "unknown"})`,
        );
      });

      await use();
      expect(failures, "browser console, page, and network failures").toEqual([]);
    },
    { auto: true },
  ],
});

export { expect } from "@playwright/test";
export type { Page } from "@playwright/test";
