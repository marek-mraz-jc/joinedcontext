import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// An App inside the Portal (T-2689, AP-122): `vite preview` has no Portal API and no static host
// behind it, so both are answered in the browser; the page, its route and its frame are real.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "demo.steward",
  name: "Demo Steward",
  email: "demo.steward@hel.fi",
  roles: ["portal-editor"],
};
const COMMIT = "4f2a9c1e0b7d3a5f6c8e9d0a1b2c3d4e5f6a7b8c";

function app(name: string, lifecycle: string) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "App",
    metadata: { name, namespace: "helsinki", title: { en: "City bikes" } },
    spec: { kind: "static", visibility: "internal", lifecycle, dataNeeds: [] },
    status: lifecycle === "published" ? { build: { commit: COMMIT, digest: `sha256:${"b".repeat(64)}` } } : {},
  };
}

// `realm` is what the realm answers the silent sign-in check (T-3034), in the fragment of the
// Portal's landing page: an error when the person must sign in, a code when the session lives.
async function stub(page: Page, realm = "error=login_required"): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    // The silent realm check (T-3034): the realm says the person must sign in.
    if (path === "/api/v1/auth/sso-check") {
      return route.fulfill({ status: 303, headers: { location: `/api/v1/auth/sso-check/done#${realm}` } });
    }
    if (path === "/api/v1/auth/sso-check/done") return route.fulfill({ status: 200, contentType: "text/html", body: "<!doctype html>" });
    if (path === "/api/v1/projects") {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    }
    if (path.endsWith("/apps/city-bikes/build") || path.endsWith("/apps/old-map/build")) {
      return json({ repositoryUrl: null, packageUrl: null, run: null, rebuild: { allowed: false, reason: "no repository" } });
    }
    if (path.endsWith("/apps/city-bikes")) return json(app("city-bikes", "published"));
    if (path.endsWith("/apps/old-map")) return json(app("old-map", "retired"));
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  // The App's own address, as the edge serves it: framed only by the Portal. A predicate, not a
  // glob: the frame asks with `?lang=` (T-4264), which a glob ending in `/` never matches.
  await page.route((url) => url.pathname === "/apps/city-bikes/", (route) =>
    route.fulfill({
      status: 200,
      contentType: "text/html",
      headers: { "content-security-policy": "default-src 'self'; frame-ancestors 'self'" },
      body: '<!doctype html><html lang="en"><head><title>City bikes</title></head><body><main><h1>Stations</h1></main></body></html>',
    }),
  );
}

test.describe("an App inside the Portal (AP-122)", () => {
  test("runs in a sandboxed frame under the Portal's header, with a window of its own offered", async ({ page }) => {
    await stub(page);
    await page.goto("/projects/helsinki/apps/city-bikes/open?lang=en");

    await expect(page.getByRole("heading", { level: 1, name: "City bikes" })).toBeVisible();
    // The commit is developer information: on the details page's badge, not here (T-2908).
    await expect(page.getByText(/4f2a9c1/)).toHaveCount(0);
    const frame = page.locator("iframe");
    await expect(frame).toHaveAttribute("title", "City bikes, the application");
    await expect(frame).toHaveAttribute(
      "sandbox",
      "allow-scripts allow-forms allow-popups allow-downloads",
    );
    await expect(page.frameLocator("iframe").getByRole("heading", { name: "Stations" })).toBeVisible();

    const own = page.getByRole("link", { name: /Open in new window/ });
    await expect(own).toHaveAttribute("href", "/apps/city-bikes/?lang=en");
    await expect(own).toHaveAttribute("target", "_blank");
    await expect(own).toHaveAttribute("rel", "noopener noreferrer");

    expect(await axeViolations(page)).toEqual([]);
  });

  // T-2908: at every width the frame takes all the window leaves under the header and the slim
  // bar, and neither the page nor the Portal around the frame scrolls.
  for (const width of [375, 768, 1440, 2560]) {
    test(`fills the window under a slim bar at ${width} px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await stub(page);
      await page.goto("/projects/helsinki/apps/city-bikes/open?lang=en");
      const frame = page.locator("iframe");
      await expect(page.frameLocator("iframe").getByRole("heading", { name: "Stations" })).toBeVisible();

      const box = await frame.boundingBox();
      const bar = await page.getByRole("heading", { level: 1, name: "City bikes" }).boundingBox();
      expect(box, "the frame is laid out").not.toBeNull();
      expect(bar, "the bar is laid out").not.toBeNull();
      if (!box || !bar) return;
      // Down to the window's bottom edge, and the full width the navigation leaves.
      expect(Math.round(box.y + box.height)).toBe(900);
      expect(box.width).toBeGreaterThan(width - 400);
      // The bar is slim: the App starts within a few lines of the Portal's header.
      expect(box.y).toBeLessThan(width < 640 ? 200 : 130);
      const scrolls = await page.evaluate(() => document.scrollingElement!.scrollHeight > window.innerHeight);
      expect(scrolls, "the page itself does not scroll").toBe(false);
      expect(await axeViolations(page)).toEqual([]);
    });
  }

  // T-2941, T-3034: the stub App carries no SDK, so it never says it is up; 8 s after its load the
  // page asks the realm silently, and the realm's login_required brings the sign-in offer above
  // the frame, readable in both themes, naming each control once.
  for (const colorScheme of ["light", "dark"] as const) {
    test(`offers the sign-in above a silent App's frame (${colorScheme})`, async ({ page }) => {
      await page.emulateMedia({ colorScheme });
      await page.clock.install();
      await stub(page);
      await page.goto("/projects/helsinki/apps/city-bikes/open?lang=en");
      await expect(page.frameLocator("iframe").getByRole("heading", { name: "Stations" })).toBeVisible();
      await expect(page.getByText("Your sign-in has ended.")).toHaveCount(0);

      await page.clock.runFor(8_000);
      const status = page.getByRole("status").filter({ hasText: "Your sign-in has ended." });
      await expect(status).toBeVisible();
      await expect(status.getByRole("button", { name: "Sign in again" })).toBeVisible();
      await expect(page.getByRole("link", { name: /Open in new window/ })).toHaveCount(1);
      await expect(page.frameLocator("iframe").getByRole("heading", { name: "Stations" })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);

      await status.getByRole("button", { name: "Hide this message" }).click();
      await expect(page.getByText("Your sign-in has ended.")).toHaveCount(0);
    });
  }

  test("a silent App whose realm session lives brings no sign-in offer", async ({ page }) => {
    await page.clock.install();
    await stub(page, "code=never-redeemed&state=x");
    await page.goto("/projects/helsinki/apps/city-bikes/open?lang=en");
    await expect(page.frameLocator("iframe").getByRole("heading", { name: "Stations" })).toBeVisible();

    const landed = page.waitForRequest((request) => request.url().includes("/api/v1/auth/sso-check/done"));
    await page.clock.runFor(8_000);
    await landed;
    // The check frame lands, is read and goes away; nothing is claimed.
    await expect(page.getByTestId("app-sso-check")).toHaveCount(0);
    await expect(page.getByText("Your sign-in has ended.")).toHaveCount(0);
  });

  test("a retired App shows its state and no frame", async ({ page }) => {
    await stub(page);
    await page.goto("/projects/helsinki/apps/old-map/open?lang=en");

    await expect(page.getByText("This application is retired")).toBeVisible();
    await expect(page.locator("iframe")).toHaveCount(0);
    await expect(page.getByRole("link", { name: /Open in new window/ })).toHaveCount(0);
  });
});
