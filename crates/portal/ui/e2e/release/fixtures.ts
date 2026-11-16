/**
 * Release e2e (T-7156): real clicks on a throwaway full jc stack. bins/jc/e2e/release/stacks.py
 * starts the stacks and writes RELEASE_STACKS (stacks.json, 0600): each stack's base URL and, per
 * Playwright slot, the persons `admin` and `approver` (/platform-admins), `viewer` (zilina's
 * viewers) and `outsider` (no group) with their passwords. Worker p runs on stack p % N as slot p / N, so two
 * workers never share a person.
 *
 * No route, mock or stub: the browser talks only to the stack (release/lint.spec.ts refuses one).
 * The second factor is enrolled by the browser on a person's first sign-in (seeds below).
 */
import { createHmac } from "node:crypto";
import { closeSync, openSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test as base, expect, type Browser, type BrowserContext, type Page, type TestInfo } from "@playwright/test";

export type Persona = "admin" | "approver" | "viewer" | "outsider";
type Person = { user: string; password: string; project: string };
type Stack = { name: string; baseURL: string; slots: Record<Persona, Person>[] };

export const ready = (page: Page) => page.getByRole("navigation", { name: "Main navigation" });

function stacks(): Stack[] {
  const file = process.env.RELEASE_STACKS;
  if (!file) throw new Error("RELEASE_STACKS is unset: start the stacks with bins/jc/e2e/release/stacks.py up");
  return (JSON.parse(readFileSync(file, "utf8")) as { stacks: Stack[] }).stacks;
}

type Seed = { key: Buffer; algorithm: string };
/**
 * Seeds enrolled in this run, by person. A worker that a failed test replaced signs its persons in
 * again and needs the seed its predecessor enrolled, so each is kept beside stacks.json in a 0600
 * file of the run's 0700 directory, which `stacks.py down` deletes; never logged or attached.
 */
const seeds = {
  file: (user: string) => join(dirname(process.env.RELEASE_STACKS ?? "."), `seed-${user.replace(/[^a-z0-9-]/gi, "_")}`),
  get(user: string): Seed | undefined {
    try {
      const [algorithm, key] = readFileSync(this.file(user), "utf8").split(":");
      return { algorithm, key: Buffer.from(key, "base64") };
    } catch {
      return undefined;
    }
  },
  set(user: string, seed: Seed): void {
    writeFileSync(this.file(user), `${seed.algorithm}:${seed.key.toString("base64")}`, { mode: 0o600 });
  },
};

/** RFC 4648 base32 (an authenticator app's key) to bytes. */
function base32(key: string): Buffer {
  let bits = "";
  for (const c of key.replace(/=+$/, "")) bits += "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567".indexOf(c).toString(2).padStart(5, "0");
  return Buffer.from(bits.match(/.{8}/g)?.map((b) => parseInt(b, 2)) ?? []);
}

/** RFC 6238, 6 digits, 30 s steps, the algorithm kaisatsu's set-up page lists (SHA1/256/512). */
export function totp(seed: Seed, step: number): string {
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(step));
  const mac = createHmac(seed.algorithm, seed.key).update(counter).digest();
  const offset = mac[mac.length - 1] & 0x0f;
  return String((mac.readUInt32BE(offset) & 0x7fffffff) % 1_000_000).padStart(6, "0");
}

/** A code kaisatsu has not taken yet: each code is single use, so the step is claimed by an
 * exclusive file in the run's directory (e2e/live/totp.ts unusedCode), so a rerun never meets an
 * old claim, and the next step is waited for when all three are taken. */
async function unusedCode(user: string, seed: Seed): Promise<string> {
  for (;;) {
    const step = Math.floor(Date.now() / 30_000);
    // kaisatsu takes the step before and after the current one too (look-ahead 1): three codes a step.
    for (const candidate of [step, step + 1, step - 1]) {
      try {
        closeSync(openSync(join(dirname(process.env.RELEASE_STACKS ?? tmpdir()), `claim-${user.replace(/[^a-z0-9.-]/gi, "_")}-${candidate}`), "wx"));
        return totp(seed, candidate);
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
      }
    }
    await new Promise((resolve) => setTimeout(resolve, (step + 1) * 30_000 - Date.now() + 250));
  }
}

/** A name no other journey, run or stack holds: `<prefix>-<base36 time><random>`, lower case. */
export function unique(prefix: string): string {
  return `${prefix}-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`.slice(0, 40);
}

/** Walks the sign-in from `path`: the Portal's Sign in, kaisatsu's password form, the authenticator
 * set-up on a first sign-in (the key read off the page) or the one-time code after it. */
export async function signIn(page: Page, who: Person, path = "/"): Promise<void> {
  // The Portal's own sign-in door (jc: /api/v1/auth/login), as the smoke ui suite enters it.
  await page.goto("/api/v1/auth/login", { waitUntil: "load" });
  // Every stack has persons of the same names: seeds and code claims are kept per stack (host).
  const holder = `${new URL(page.url()).host}-${who.user}`;
  const button = page.getByRole("button", { name: "Sign in" });
  for (let step = 0; step < 6; step += 1) {
    // Back on the Portal (not kaisatsu's pages, not the sign-in door): the walk is over.
    if (step > 0 && !/^\/(login\/realms\/|api\/v1\/auth\/)/.test(new URL(page.url()).pathname)) break;
    await page
      .locator("#username, #otp, #kc-totp-secret-key")
      .or(button)
      .or(ready(page))
      .first()
      .waitFor({ timeout: 30_000 })
      .catch(() => undefined);
    if (await page.locator("#username").count()) {
      await page.fill("#username", who.user);
      await page.fill("#password", who.password);
      await Promise.all([page.waitForEvent("load"), page.click("#kc-login")]);
      continue;
    }
    if (await page.locator("#kc-totp-secret-key").count()) {
      // "Unable to scan?" shows the key, as a person without a camera reads it.
      if (await page.locator("#mode-manual").isVisible()) {
        await page.click("#mode-manual");
        await page.locator("#kc-totp-secret-key").waitFor({ state: "visible" });
      }
      const key = ((await page.locator("#kc-totp-secret-key").textContent()) ?? "").replace(/\s+/g, "");
      if (!/^[A-Z2-7]{16,}=*$/.test(key)) throw new Error(`the authenticator set-up page of ${who.user} shows no base32 key`);
      const listed = (await page.locator("#kc-totp-manual").innerText()).match(/\bSHA(1|256|512)\b/);
      const seed = { key: base32(key), algorithm: `sha${listed?.[1] ?? "1"}` };
      seeds.set(holder, seed);
      if (await page.locator("input[name=userLabel]").count()) await page.fill("input[name=userLabel]", "release-e2e");
      await page.fill("input[name=otp]", await unusedCode(holder, seed));
      await Promise.all([page.waitForEvent("load"), page.locator("input[name=otp]").press("Enter")]);
      continue;
    }
    if (await page.locator("#otp").count()) {
      const seed = seeds.get(holder);
      if (!seed) throw new Error(`${who.user} has a second factor this run did not enrol: stacks.py persons resets it`);
      await page.fill("#otp", await unusedCode(holder, seed));
      await Promise.all([page.waitForEvent("load"), page.click("#kc-login")]);
      continue;
    }
    if (page.url().includes("/login") && (await button.count())) {
      await Promise.all([page.waitForEvent("load"), button.first().click()]);
      continue;
    }
    break;
  }
  // Signed in when the Portal knows the person: /api/v1/auth/me answers 200 on the session cookie.
  // Where it stopped, without the query (kaisatsu's holds its state) and never a shot: the set-up
  // page shows the seed.
  // The page's own fetch: Playwright's request client drops the Secure __Host- cookies Chromium
  // keeps on http://127.0.0.1 (bins/jc/smoke/common.py browser_get).
  const me = await page.evaluate(() => fetch("/api/v1/auth/me").then((r) => r.status));
  if (me !== 200) {
    const { origin, pathname } = new URL(page.url());
    const text = (await page.locator("body").innerText({ timeout: 2_000 }).catch(() => "")).replace(/\s+/g, " ").slice(0, 160);
    throw new Error(`sign-in of ${who.user} stopped on ${origin}${pathname} ("${text}"), /api/v1/auth/me ${me}`);
  }
  if (path !== "/" || new URL(page.url()).pathname !== "/") await page.goto(path, { waitUntil: "load" });
}

/** Desktop 1440 and phone 400 shots of the page's current state, attached to the test. */
export async function shoot(page: Page, info: TestInfo, state: string): Promise<void> {
  const size = page.viewportSize() ?? { width: 1440, height: 900 };
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
    await page.waitForTimeout(150);
    const path = info.outputPath(`${state}-${width}.png`);
    await page.screenshot({ path, fullPage: false });
    await info.attach(`${state}-${width}`, { path, contentType: "image/png" });
  }
  await page.setViewportSize(size);
}

type Workers = { stack: Stack; slot: Record<Persona, Person>; sessions: Map<Persona, object> };
type Tests = {
  person: (who: Persona) => Person;
  as: (who: Persona, path?: string) => Promise<Page>;
};

async function session(browser: Browser, baseURL: string, who: Person): Promise<object> {
  const context = await browser.newContext({ baseURL });
  const page = await context.newPage();
  await signIn(page, who, "/settings");
  const state = await context.storageState();
  await context.close();
  return state;
}

export const test = base.extend<Tests, Workers>({
  stack: [
    // Playwright reads a fixture's dependencies from its first parameter, which must destructure.
    async ({ playwright: _playwright }, provide, info) => {
      const all = stacks();
      await provide(all[info.parallelIndex % all.length]);
    },
    { scope: "worker" },
  ],
  slot: [
    async ({ stack }, provide, info) => {
      const n = stacks().length;
      const slot = stack.slots[Math.floor(info.parallelIndex / n)];
      if (!slot) throw new Error(`stack ${stack.name} has no person slot ${Math.floor(info.parallelIndex / n)}: start it with --slots >= workers / stacks`);
      await provide(slot);
    },
    { scope: "worker" },
  ],
  // Every person of the slot signs in once per worker, before the first test: a worker-scoped
  // auto fixture runs ahead of the test's trace recorder, so no trace holds a password or a code.
  // The tests open contexts from the stored sessions (cookies only).
  sessions: [
    async ({ browser, stack, slot }, provide) => {
      const all = ["admin", "approver", "viewer", "outsider"] as const;
      const states = await Promise.all(all.map((who) => session(browser, stack.baseURL, slot[who])));
      await provide(new Map(all.map((who, i) => [who, states[i]])));
    },
    { scope: "worker", auto: true, timeout: 180_000 },
  ],
  baseURL: async ({ stack }, provide) => provide(stack.baseURL),
  person: async ({ slot }, provide) => provide((who) => slot[who]),
  as: async ({ browser, stack, sessions }, provide) => {
    const opened: BrowserContext[] = [];
    await provide(async (who, path = "/") => {
      const context = await browser.newContext({
        baseURL: stack.baseURL,
        storageState: sessions.get(who) as never,
        viewport: { width: 1440, height: 900 },
      });
      opened.push(context);
      const page = await context.newPage();
      await page.goto(path, { waitUntil: "load" });
      return page;
    });
    for (const c of opened) await c.close();
  },
});

export { expect };
