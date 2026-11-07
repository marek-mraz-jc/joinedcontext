import { execFile } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { expect } from "@playwright/test";
import type { Browser, BrowserContext, Page } from "@playwright/test";
import { answerOtp, submitted } from "./totp";

const run = promisify(execFile);

/** The forge organization of the configuration (PF-85); the bootstrap names it `joinedcontext`. */
export const FORGE_ORG = process.env.JC_FORGE_ORG ?? "joinedcontext";
/** The organization repository of layout 2 (PF-86): it kept the old repository's name. */
export const ORG_REPO = process.env.JC_FORGE_ORG_REPO ?? "configuration";

/**
 * The forge's public URL, the Portal's `JC_GITEA_PUBLIC_URL` (`https://{domain}/git`). It is not on
 * the Portal's host: on dev `portal.dev…/git/…` is a Portal route that redirects to its login. By
 * default the Portal URL's host without its `portal.` label, so `https://portal.dev.joinedcontext.com`
 * gives `https://dev.joinedcontext.com/git`.
 */
export const FORGE_URL = (process.env.JC_FORGE_URL ?? defaultForgeUrl(process.env.PORTAL_URL ?? "https://portal.dev.joinedcontext.com")).replace(/\/+$/, "");

export function defaultForgeUrl(portalUrl: string): string {
  const portal = new URL(portalUrl);
  portal.hostname = portal.hostname.replace(/^portal\./, "");
  return new URL("/git", portal.origin).toString();
}

/** An absolute forge URL for a path below the forge root, e.g. `/api/v1/user`. */
export function forge(path: string): string {
  return `${FORGE_URL}${path}`;
}

/** A page URL on the forge itself, and not a Portal URL that merely contains `/git/`. */
const ON_FORGE = new RegExp(`^${FORGE_URL.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}/`);

/** A person signed in to the forge with their own identity, and the forge token they made there. */
export interface ForgePerson {
  context: BrowserContext;
  page: Page;
  login: string;
  token: string;
  tokenName: string;
}

/**
 * Signs one browser context in to the forge through its Keycloak button, the way a person does
 * (PF-79), and has them make a repository token on their own settings page (PF-87: git runs over
 * HTTPS with the person's own forge credential). The token never reaches a log: it is handed to
 * git through the environment, never on a command line or in a URL.
 */
export async function forgeSignIn(
  browser: Browser,
  who: { user: string; password: string; totp?: string },
  tokenName: string,
): Promise<ForgePerson> {
  if (!who.password) {
    throw new Error(`no password in the environment for ${who.user}`);
  }
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.goto(forge("/user/oauth2/keycloak"), { waitUntil: "load" });
  if (await page.locator("#username").count()) {
    await page.fill("#username", who.user);
    await page.fill("#password", who.password);
    await submitted(page, () => page.click("#kc-login"));
    await answerOtp(page, who);
  }
  await expect(page, `${who.user} is back in the forge after the Keycloak login`).toHaveURL(ON_FORGE, { timeout: 60_000 });

  const token = await makeToken(page, who.user, tokenName);
  const user = await page.request.get(forge("/api/v1/user"), { headers: { authorization: `token ${token}` } });
  expect(user.ok(), `the forge reads ${who.user}'s own account with the new token`).toBe(true);
  const login = ((await user.json()) as { login: string }).login;
  return { context, page, login, token, tokenName };
}

/** Revokes the token the journey made and closes the session; nothing of the journey stays in the forge. */
export async function forgeSignOut(person: ForgePerson | null): Promise<void> {
  if (!person) return;
  try {
    await revokeToken(person.page, person.tokenName);
  } finally {
    await person.context.close();
  }
}

/**
 * Makes a repository token on the signed-in person's own settings page by filling the form and
 * pressing its button, as the person would. Gitea 1.25+ renders no `_csrf` and refuses a
 * cross-origin form post by its Fetch Metadata instead, so the journey drives the page rather than
 * replaying a form. The token is shown once, in the info flash; `read:user` lets the journey read
 * the person's own login with it.
 */
export async function makeToken(page: Page, who: string, tokenName: string): Promise<string> {
  await page.goto(forge("/user/settings/applications"), { waitUntil: "load" });
  const name = page.locator("input#name");
  if (!(await name.isVisible())) {
    await page.getByText("Generate New Token", { exact: true }).click();
  }
  await name.fill(tokenName);
  await page.locator('input[name="scope-repository"][value="write:repository"]').check();
  await page.locator('input[name="scope-user"][value="read:user"]').check();
  await page.getByRole("button", { name: "Generate Token" }).click();
  const shown = await page.locator(".flash-info").first().textContent({ timeout: 15_000 }).catch(() => null);
  const token = /\b([0-9a-f]{40})\b/.exec(shown ?? "")?.[1];
  if (!token) {
    const said = (await page.locator(".flash-error, .flash-warning").first().textContent({ timeout: 1_000 }).catch(() => null))?.trim();
    throw new Error(`the forge made no token for ${who}: ${said || `no token on ${page.url()}`}`);
  }
  return token;
}

/** Deletes the person's token named `tokenName` through its Delete button and the confirmation. */
export async function revokeToken(page: Page, tokenName: string): Promise<void> {
  await page.goto(forge("/user/settings/applications"), { waitUntil: "load" });
  const row = page.locator(".item").filter({ has: page.locator(".item-title").getByText(tokenName, { exact: true }) });
  if (!(await row.count())) return;
  await row.getByRole("button", { name: "Delete" }).click();
  // Fomantic moves the modal into its dimmer, out of `#delete-token`; the open one is the only "Yes".
  await page.getByRole("button", { name: "Yes", exact: true }).click();
  await expect(row, `the forge no longer lists ${tokenName}`).toHaveCount(0);
}

/** The forge's own answer to a person reading a repository over git's HTTP protocol: 200 or 404. */
export async function gitReadStatus(person: ForgePerson, repo: string): Promise<number> {
  const answer = await fetch(forge(`/${FORGE_ORG}/${repo}.git/info/refs?service=git-upload-pack`), {
    headers: { authorization: `Basic ${Buffer.from(`${person.login}:${person.token}`).toString("base64")}` },
  });
  return answer.status;
}

/** Runs git with the person's forge credential in the environment (never argv, never a URL). */
export async function git(person: ForgePerson, cwd: string, args: string[]): Promise<string> {
  const header = `Authorization: Basic ${Buffer.from(`${person.login}:${person.token}`).toString("base64")}`;
  try {
    const { stdout } = await run("git", args, {
      cwd,
      env: {
        ...process.env,
        GIT_TERMINAL_PROMPT: "0",
        GIT_CONFIG_COUNT: "3",
        GIT_CONFIG_KEY_0: "http.extraheader",
        GIT_CONFIG_VALUE_0: header,
        GIT_CONFIG_KEY_1: "user.name",
        GIT_CONFIG_VALUE_1: person.login,
        GIT_CONFIG_KEY_2: "user.email",
        GIT_CONFIG_VALUE_2: `${person.login}@journeys.joinedcontext.com`,
      },
      maxBuffer: 16 * 1024 * 1024,
    });
    return stdout;
  } catch (error) {
    // execFile's message repeats the command line, which holds no secret: the header is in env.
    const failed = error as { stderr?: string; message?: string };
    throw new Error(`git ${args[0]} failed: ${failed.stderr?.trim() || failed.message}`);
  }
}

/** A clone of one project repository in a directory of its own, removed by `done`. */
export async function cloneRepository(person: ForgePerson, repo: string): Promise<{ dir: string; done: () => Promise<void> }> {
  const dir = await mkdtemp(join(tmpdir(), `t2647-${repo}-`));
  await git(person, dir, ["clone", "--quiet", forge(`/${FORGE_ORG}/${repo}.git`), "."]);
  return { dir, done: () => rm(dir, { recursive: true, force: true }) };
}

/** One forge API call in the person's name. */
export async function forgeApi(person: ForgePerson, method: "GET" | "POST" | "DELETE", path: string, data?: unknown) {
  return person.page.request.fetch(forge(`/api/v1${path}`), {
    method,
    headers: { authorization: `token ${person.token}`, "content-type": "application/json" },
    data: data === undefined ? undefined : JSON.stringify(data),
  });
}
