import { readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { BOOT_RETRY, goSignedIn } from "./live/portal";

// The edge's minute is the live wait; the stub has no rate window to sit out.
BOOT_RETRY.waitMs = 1_000;

// T-5068: the walker's viewer sign-in met the shell's "The Portal did not load" notice (the entry
// script failed during a rollout) and waited 60 s for a navigation that never came. The real
// index.html with its entry script refused stands in for that load; the reload answers the Portal.
test("the live sign-in reloads a Portal whose entry script did not load", async ({ page }) => {
  test.setTimeout(60_000);
  const shell = readFileSync("index.html", "utf8");
  let documents = 0;
  await page.route("http://portal.test/**", (route) => {
    if (route.request().resourceType() !== "document") return route.fulfill({ status: 429 });
    documents += 1;
    const body = documents === 1 ? shell : '<nav aria-label="Main navigation"><a href="/">Home</a></nav>';
    return route.fulfill({ body, contentType: "text/html" });
  });
  await goSignedIn(page, { user: "nobody", password: "unused" }, "http://portal.test/projects");
  await expect(page.getByRole("navigation", { name: "Main navigation" })).toBeVisible();
  expect(documents).toBe(2);
});

// T-5176: the steward's walk stopped on Keycloak's one-time-code form. Each submit waited with
// waitForURL(() => true), which matches the page being left, so the next step looked at the
// password form again while Keycloak was still answering and the TOTP form came after the walk
// stopped looking. On dev the OTP page's URL was there before its form; here a slow script ahead of
// the form holds the parser the same way.
test("the live sign-in answers a TOTP form that loads after the password", async ({ page }) => {
  test.setTimeout(60_000);
  const posts: string[] = [];
  const form = (action: string, fields: string) =>
    `<form method="post" action="${action}">${fields}<input type="submit" id="kc-login" value="Sign in"></form>`;
  let signedIn = false;
  await page.route("http://portal.test/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === "/kc/slow.js") {
      await new Promise((resolve) => setTimeout(resolve, 2_000));
      return route.fulfill({ body: "", contentType: "text/javascript" });
    }
    if (request.resourceType() !== "document") return route.fulfill({ status: 404 });
    if (request.method() === "POST") posts.push(`${path} ${request.postData() ?? ""}`);
    if (path === "/kc/password") {
      const otp = form("/kc/otp", '<input id="otp" name="otp">');
      return route.fulfill({ body: `<script src="/kc/slow.js"></script>${otp}`, contentType: "text/html" });
    }
    if (path === "/kc/otp") {
      // Playwright routes only the first request of a redirect chain: answer the landing page itself.
      signedIn = /^otp=\d{6}$/.test(request.postData() ?? "");
    }
    const body = signedIn
      ? '<nav aria-label="Main navigation"><a href="/">Home</a></nav>'
      : form("/kc/password", '<input id="username" name="username"><input id="password" name="password">');
    return route.fulfill({ body, contentType: "text/html" });
  });
  await goSignedIn(
    page,
    { user: `journey-${Date.now()}@hel.fi`, password: "unused", totp: "12345678901234567890123456789012" },
    "http://portal.test/projects",
  );
  await expect(page.getByRole("navigation", { name: "Main navigation" })).toBeVisible();
  expect(posts.map((post) => post.split(" ")[0])).toEqual(["/kc/password", "/kc/otp"]);
});

test("the live sign-in still fails a Portal that never boots", async ({ page }) => {
  test.setTimeout(240_000);
  const shell = readFileSync("index.html", "utf8");
  let documents = 0;
  await page.route("http://portal.test/**", (route) => {
    if (route.request().resourceType() !== "document") return route.fulfill({ status: 429 });
    documents += 1;
    return route.fulfill({ body: shell, contentType: "text/html" });
  });
  await expect(goSignedIn(page, { user: "nobody", password: "unused" }, "http://portal.test/projects")).rejects.toThrow(
    /Main navigation/,
  );
  expect(documents).toBe(3);
});
