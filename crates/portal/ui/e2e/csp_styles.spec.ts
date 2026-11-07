import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { randomBytes } from "node:crypto";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-5828: the Portal's style-src names no 'unsafe-inline'. `vite preview` sends no policy, so this
// journey serves the shell as the Portal does (src/csp.rs): a fresh nonce written over the build's
// slot and the same style-src in the header. Monaco and Radix's scroll lock write <style> elements
// at run time (src/cspNonce.ts stamps them); any style the browser refuses fails the journey.

const API = "joinedcontext.com/v1alpha1";
const dir = join("test-results", "T-5828");
const SLOT = "__JC_CSP_NONCE__";

const BINDING = {
  apiVersion: API,
  kind: "RoleBinding",
  metadata: { name: "stewards", namespace: "org", title: { en: "Helsinki stewards" } },
  spec: { subjects: [{ user: "eva@hel.fi" }], role: "steward", scope: { project: "helsinki" } },
  status: { phase: "Live" },
};

/** Serves every page under the Portal's own style policy and records what the browser refused. */
async function portalPolicy(page: Page) {
  await page.addInitScript(() => {
    const refused: string[] = [];
    (window as unknown as { cspRefused: string[] }).cspRefused = refused;
    document.addEventListener("securitypolicyviolation", (event) => {
      refused.push(`${event.effectiveDirective} ${event.blockedURI} ${event.sample}`);
    });
  });
  await page.route("**/*", async (route) => {
    if (route.request().resourceType() !== "document") return route.fallback();
    const response = await route.fetch();
    const nonce = randomBytes(16).toString("base64");
    const body = (await response.text()).replaceAll(SLOT, nonce);
    return route.fulfill({
      response,
      body,
      headers: {
        ...response.headers(),
        "content-security-policy":
          "default-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'self'; img-src 'self' data: blob:; " +
          `style-src 'self' 'nonce-${nonce}'; style-src-attr 'unsafe-inline'; font-src 'self' data:; worker-src 'self' blob:`,
      },
    });
  });
}

async function stub(page: Page) {
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "eva", name: "Eva", email: "eva@hel.fi", roles: ["portal-editor"] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "org", bootstrap: false, grants: [{ role: "viewer", binding: "b", rule: { kinds: ["RoleBinding", "Role", "Project"], verbs: ["read"] } }] });
    }
    if (path === "/api/v1/kinds/RoleBinding/schema") return json({ type: "object" });
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "List", items: [{ name: "helsinki" }] });
    if (path === "/api/v1/projects/org/rolebindings/stewards") return json(BINDING);
    if (path === "/api/v1/projects/org/rolebindings") return json({ apiVersion: API, kind: "List", items: [BINDING] });
    return json({ apiVersion: API, kind: "List", items: [] });
  });
}

const refused = (page: Page) => page.evaluate(() => (window as unknown as { cspRefused: string[] }).cspRefused);

for (const width of [1440, 400]) {
  test(`Monaco and a dialog draw under the Portal's style policy, at ${width} px`, async ({ page }) => {
    mkdirSync(dir, { recursive: true });
    await page.setViewportSize({ width, height: 900 });
    await portalPolicy(page);
    await stub(page);
    await page.goto("/projects/helsinki/settings/members?edit=stewards&lang=en");
    const editor = page.locator(".monaco-editor").first();
    await expect(editor).toContainText("apiVersion");
    // Monaco's token colours live in a <style> it writes at run time: keys and values differ.
    const key = editor.locator(".view-line span span", { hasText: "apiVersion" }).first();
    await expect.poll(async () => key.evaluate((span) => getComputedStyle(span).color)).not.toBe("rgb(0, 0, 0)");
    // Its view lines sit where their style="top:..." attributes put them.
    const lines = editor.locator(".view-line");
    await expect.poll(async () => (await lines.nth(1).boundingBox())?.y ?? 0).toBeGreaterThan((await lines.nth(0).boundingBox())?.y ?? 0);
    await page.screenshot({ path: join(dir, `monaco-${width}.png`) });

    // The command palette is a Radix Dialog: its scroll lock writes a <style> into <head>.
    await page.keyboard.press("Control+k");
    await expect(page.getByRole("dialog")).toBeVisible();
    await expect.poll(() => page.evaluate(() => getComputedStyle(document.body).overflow)).toBe("hidden");
    await page.screenshot({ path: join(dir, `dialog-${width}.png`) });

    expect(await refused(page)).toEqual([]);
  });
}

test("a style element injected as markup is refused", async ({ page }) => {
  await portalPolicy(page);
  await stub(page);
  await page.goto("/projects/helsinki/settings/members?edit=stewards&lang=en");
  await expect(page.locator(".monaco-editor").first()).toBeVisible();
  await page.evaluate(() => {
    document.body.insertAdjacentHTML("beforeend", "<style>body{background:rgb(1, 2, 3)}</style>");
  });
  await expect.poll(async () => (await refused(page)).some((line) => line.startsWith("style-src-elem inline"))).toBe(true);
  expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).not.toBe("rgb(1, 2, 3)");
});
