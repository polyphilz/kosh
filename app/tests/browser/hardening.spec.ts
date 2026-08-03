import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "./fixtures";

const primaryRoutes = [
  { path: "/#/", heading: "Note" },
  { path: "/#/settings", heading: "Settings" },
] as const;

for (const appearance of ["LIGHT", "DARK"] as const) {
  test(`primary routes have no serious accessibility violations in ${appearance.toLowerCase()} mode`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1280, height: 900 });
    for (const route of primaryRoutes) {
      await page.goto(route.path);
      await page.evaluate((value) => {
        document.documentElement.dataset.appearance = value;
      }, appearance);
      await expect(page.getByRole("heading", { name: route.heading })).toBeVisible();
      const results = await new AxeBuilder({ page })
        .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
        .analyze();
      expect(results.violations, `${appearance} ${route.path} accessibility violations`).toEqual(
        [],
      );
    }
    await page.goto("/#/");
    await page.evaluate((value) => {
      document.documentElement.dataset.appearance = value;
    }, appearance);
    await page.keyboard.press("Meta+k");
    await expect(page.getByRole("dialog", { name: "Search notes" })).toBeVisible();
    const searchResults = await new AxeBuilder({ page })
      .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
      .analyze();
    expect(
      searchResults.violations,
      `${appearance} search overlay accessibility violations`,
    ).toEqual([]);
  });
}

test("all primary destinations are reachable in order without a pointer", async ({ page }) => {
  await page.goto("/#/settings");
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();

  await page.keyboard.press("Tab");
  await expect(page.getByRole("button", { name: "Hide sidebar" })).toBeFocused();
  for (const [role, name] of [
    ["button", "New note"],
    ["button", "Search"],
    ["link", "Settings"],
  ] as const) {
    await page.keyboard.press("Tab");
    await expect(
      page.getByRole("navigation", { name: "Primary" }).getByRole(role, {
        name,
        exact: true,
      }),
    ).toBeFocused();
  }

  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/#\/settings$/);
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
});

test("minimum supported window reflows at 200 percent text without hidden controls", async ({
  page,
}) => {
  await page.setViewportSize({ width: 720, height: 700 });
  await page.goto("/#/");
  await page.evaluate(() => {
    document.documentElement.style.fontSize = "200%";
  });

  const navigation = page.getByRole("navigation", { name: "Primary" });
  await expect(navigation.getByRole("button", { name: "New note", exact: true })).toBeVisible();
  await expect(navigation.getByRole("button", { name: "Search", exact: true })).toBeVisible();
  await expect(navigation.getByRole("link", { name: "Settings", exact: true })).toBeVisible();
  const overflow = await page.evaluate(() => ({
    viewport: document.documentElement.clientWidth,
    content: document.documentElement.scrollWidth,
  }));
  expect(overflow.content).toBeLessThanOrEqual(overflow.viewport + 1);
});

test("reduced-motion preference removes sustained animation and transition", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/#/");
  await expect(page.getByRole("heading", { name: "Note" })).toBeVisible();
  await expect(
    page.evaluate(() => matchMedia("(prefers-reduced-motion: reduce)").matches),
  ).resolves.toBe(true);

  const violations = await page.evaluate(() => {
    const durationMs = (value: string) =>
      value.split(",").map((part) => {
        const duration = part.trim();
        return duration.endsWith("ms")
          ? Number.parseFloat(duration)
          : Number.parseFloat(duration) * 1_000;
      });
    return [...document.querySelectorAll("*")]
      .map((element) => {
        const style = getComputedStyle(element);
        return {
          tag: element.tagName,
          animation: durationMs(style.animationDuration),
          iterations: style.animationIterationCount,
          transition: durationMs(style.transitionDuration),
        };
      })
      .filter(
        ({ animation, iterations, transition }) =>
          (iterations.split(",").some((value) => value.trim() === "infinite") &&
            animation.some((duration) => duration > 0.1)) ||
          transition.some((duration) => duration > 0.1),
      );
  });
  expect(violations).toEqual([]);
});

test("high-density rendering keeps focus targets and accessible names", async ({ page }) => {
  expect(await page.evaluate(() => window.devicePixelRatio)).toBe(2);
  await page.goto("/#/");
  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.focus();
  await expect(editor).toBeFocused();
  await expect(page.getByRole("button", { name: "Sources" })).toBeVisible();
  const results = await new AxeBuilder({ page }).analyze();
  expect(results.violations).toEqual([]);
});
