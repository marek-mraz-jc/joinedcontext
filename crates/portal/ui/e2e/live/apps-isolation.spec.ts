/**
 * One App attacking another on dev (T-4915, AP-12, AP-29, AP-84, AP-122, AP-133).
 *
 * Every App is served on a host of its own, `{name}.apps.{domain}`, with a host-only session
 * cookie, a host-only double-submit cookie and a policy that keeps its page on its own origin.
 * Here the viewer has both sample Apps open, and the bikes page plays the hostile App: the
 * script below runs in its origin, as its own bundle would, and tries what such a bundle could.
 * Each attempt must fail, first at the browser (same-origin policy, CSP) and then, with the
 * page's policy switched off, at the edge and the Portal, so a manifest that widens its own
 * `connect-src` still reaches nothing of another App or of the Portal.
 *
 * Nothing here writes: every request the hostile page sends is one the server refuses.
 */
import { expect, test } from "@playwright/test";
import type { BrowserContext, Page } from "@playwright/test";
import { VIEWER, goSignedIn, signIn } from "./portal";

const PROJECT = "helsinki";
const PORTAL = new URL(process.env.PORTAL_URL ?? "https://portal.dev.joinedcontext.com").origin;
/** `{name}.apps.{domain}`: the Portal's host with `portal.` swapped for `{name}.apps.`. */
const appOrigin = (name: string): string => PORTAL.replace("://portal.", `://${name}.apps.`);
const HOSTILE = appOrigin("helsinki-bikes");
const VICTIM = appOrigin("helsinki-alerts");
/** A function the victim has; the refusals come before the runtime looks it up. */
const VICTIM_FUNCTION = `${VICTIM}/api/functions/summary`;
const MARKER = "t4915-victim-secret";

test.setTimeout(600_000);

/** Opens the victim, leaves a value in its storage, then opens the hostile App's page. */
async function bothOpen(context: BrowserContext, page: Page): Promise<string> {
  await goSignedIn(page, VIEWER, `${VICTIM}/`, (opened) => opened.locator("body"));
  expect(new URL(page.url()).origin, "the victim opens on its own host").toBe(VICTIM);
  await expect(page.locator("#jc-config")).toBeAttached();
  await page.evaluate((marker) => {
    localStorage.setItem("t4915", marker);
    sessionStorage.setItem("t4915", marker);
  }, MARKER);
  const csrf = (await context.cookies(VICTIM)).find((cookie) => cookie.name === "__Host-jc_csrf");
  expect(csrf, "the victim's index set its double-submit cookie").toBeTruthy();
  await goSignedIn(page, VIEWER, `${HOSTILE}/`, (opened) => opened.locator("body"));
  expect(new URL(page.url()).origin).toBe(HOSTILE);
  return csrf?.value ?? "";
}

test("an App reads no other App's cookies or storage and the browser stops its cross-origin calls", async ({
  browser,
}) => {
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/apps?lang=en`);
  try {
    const { context, page } = viewer;
    const victimCsrf = await bothOpen(context, page);

    // Every cookie the hostile page can see is its own: no session cookie (HttpOnly), and no
    // value of the victim's or the Portal's double-submit token (host-only `__Host-`).
    const seen = await page.evaluate(() => document.cookie);
    const portalCsrf = (await context.cookies(PORTAL)).find((cookie) => cookie.name === "__Host-jc_csrf")?.value;
    expect(seen).not.toMatch(/jc_app_|jc_edge/);
    expect(seen).not.toContain(victimCsrf);
    if (portalCsrf) expect(seen).not.toContain(portalCsrf);
    // Storage is per origin.
    const stored = await page.evaluate(() => [localStorage.getItem("t4915"), sessionStorage.getItem("t4915")]);
    expect(stored).toEqual([null, null]);

    // The page's own policy keeps every call on its origin: the victim's function, its
    // services and the Portal's API never leave the browser.
    const outcomes = await page.evaluate(
      async ([fn, victim, portal]) => {
        const attempt = async (url: string, init: RequestInit): Promise<string> => {
          try {
            const answer = await fetch(url, { credentials: "include", ...init });
            return `answered ${answer.status}`;
          } catch {
            return "blocked";
          }
        };
        return [
          await attempt(fn, { method: "POST", headers: { "content-type": "application/json" }, body: "{}" }),
          await attempt(`${victim}/api/services/email/send`, { method: "POST", body: "{}" }),
          await attempt(`${portal}/api/v1/projects`, {}),
        ];
      },
      [VICTIM_FUNCTION, VICTIM, PORTAL],
    );
    expect(outcomes).toEqual(["blocked", "blocked", "blocked"]);

    // Nor does it frame the victim or the Portal: its `frame-src` is itself, and each of them
    // names who may frame it (the Portal alone, or nobody).
    for (const target of [`${VICTIM}/`, `${PORTAL}/`]) {
      const answer = await page.request.get(target, { maxRedirects: 0 });
      const policy = answer.headers()["content-security-policy"] ?? "";
      expect(policy, `${target} names its framers`).toMatch(/frame-ancestors [^;]+/);
      expect(policy).not.toContain(HOSTILE);
    }
    await info(page);
  } finally {
    await viewer.context.close();
  }
});

test("with the page's policy off, the edge and the Portal still refuse every cross-App call", async ({ browser }) => {
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/apps?lang=en`);
  // The same person and sessions in a browser that ignores the hostile page's CSP: what a
  // manifest that lists the victim or the Portal in its own `connect-src` would get.
  const state = await viewer.context.storageState();
  const bypassed = await browser.newContext({ storageState: state, bypassCSP: true });
  try {
    const page = await bypassed.newPage();
    await bothOpen(bypassed, page);

    // A credentialed CORS call gets no answer the page may read: neither the victim nor the
    // Portal sends `Access-Control-Allow-Origin` for another origin.
    const read = await page.evaluate(
      async ([fn, portal]) => {
        const attempt = async (url: string, init: RequestInit): Promise<string> => {
          try {
            const answer = await fetch(url, { credentials: "include", ...init });
            return `read ${answer.status}`;
          } catch {
            return "blocked";
          }
        };
        return [
          await attempt(fn, {
            method: "POST",
            headers: { "content-type": "application/json", authorization: "Bearer planted" },
            body: "{}",
          }),
          await attempt(`${portal}/api/v1/projects`, {}),
          await attempt(`${portal}/api/v1/projects/helsinki/apps`, { method: "POST", body: "{}" }),
        ];
      },
      [VICTIM_FUNCTION, PORTAL],
    );
    expect(read).toEqual(["blocked", "blocked", "blocked"]);

    // A form-like call needs no CORS and rides on the victim's cookies, so the server decides:
    // without the victim's double-submit token it is refused before anything runs.
    const refused = page.waitForResponse((answer) => answer.url() === VICTIM_FUNCTION);
    await page.evaluate(async (fn) => {
      await fetch(fn, { method: "POST", mode: "no-cors", credentials: "include", body: "{}" }).catch(() => undefined);
    }, VICTIM_FUNCTION);
    expect((await refused).status()).toBe(403);

    // The same call with a bearer beside the cookie: the edge still attaches the victim's
    // session token, so the bearer waives no double-submit check (T-4915).
    const planted = await bypassed.request.post(VICTIM_FUNCTION, {
      headers: { authorization: "Bearer planted", "content-type": "application/json" },
      data: {},
      maxRedirects: 0,
    });
    expect(planted.status(), await planted.text()).toBe(403);
    // And the hostile App's own token is not the victim's.
    const hostileCsrf = (await bypassed.cookies(HOSTILE)).find((cookie) => cookie.name === "__Host-jc_csrf")?.value;
    const crossed = await bypassed.request.post(VICTIM_FUNCTION, {
      headers: { "x-csrf-token": hostileCsrf ?? "none", "content-type": "application/json" },
      data: {},
      maxRedirects: 0,
    });
    expect(crossed.status(), await crossed.text()).toBe(403);
    // A service write is refused the same way.
    const service = await bypassed.request.post(`${VICTIM}/api/services/email/send`, {
      headers: { "content-type": "application/json" },
      data: {},
      maxRedirects: 0,
    });
    expect(service.status(), await service.text()).toBe(403);
    await info(page);
  } finally {
    await bypassed.close();
    await viewer.context.close();
  }
});

async function info(page: Page): Promise<void> {
  await test.info().attach("hostile-page.png", { body: await page.screenshot(), contentType: "image/png" });
}
