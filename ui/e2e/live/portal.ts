import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { expect } from "@playwright/test";
import type { APIRequestContext, Browser, BrowserContext, BrowserContextOptions, Locator, Page, Request, Response } from "@playwright/test";
import { axeViolations } from "../axe";
import { blankDocument, edgeRetryMs, pageFindings, viewerWriteFindings } from "../../tests/pageChecks";
import { type ListedChange, changeSubject } from "./staleChanges";
import { shareEdge } from "./edgeShare";
import { openedTwice } from "./tab";
import { answerOtp, submitted } from "./totp";

/**
 * The project sections the installation hides (`hiddenSections` of `/api/v1/branding`, T-2874):
 * a case over one is excused while it is hidden, and runs again the day it is shown.
 */
export async function hiddenSections(request: APIRequestContext): Promise<string[]> {
  const answer = await request.get("/api/v1/branding");
  const hidden = answer.ok() ? ((await answer.json()) as { hiddenSections?: unknown }).hiddenSections : undefined;
  return Array.isArray(hidden) ? hidden.filter((section): section is string => typeof section === "string") : [];
}

/** Whether a Portal address lies in one of the `hidden` sections. */
export function inHiddenSection(address: string, hidden: string[]): boolean {
  const section = address.split(/[?#]/)[0].split("/")[3];
  return section !== undefined && hidden.includes(section);
}

/**
 * Sends the live config's journey marker (`X-JC-Run-Origin`, AG-93) to the Portal's own host only.
 * Any other origin a page reads (the API host's MCP route, a basemap, a framed App) would have to
 * allow the header in its CORS preflight, so the browser blocked those reads and the walk judged a
 * page no person sees (T-5025). `context.request` bypasses routes and so carries no marker after this.
 */
export async function journeyMarkerOnPortalOnly(context: BrowserContext, portalHost: string): Promise<void> {
  await context.setExtraHTTPHeaders({});
  // A route turns the browser's HTTP cache off: every page the walker opened fetched the whole
  // bundle again and emptied the edge's per-IP budget, so a chunk answered 429 where a person's
  // browser reads its cached copy (T-5744). An immutable file is kept here once it loaded.
  const kept = new Map<string, { status: number; headers: Record<string, string>; body: Buffer }>();
  await context.route(
    (url) => url.host === portalHost,
    async (route) => {
      const request = route.request();
      const headers = { ...request.headers(), "x-jc-run-origin": "journey" };
      if (request.method() !== "GET" || !new URL(request.url()).pathname.startsWith("/assets/")) return route.fallback({ headers });
      const cached = kept.get(request.url());
      if (cached) return route.fulfill(cached);
      const response = await route.fetch({ headers }).catch(() => undefined);
      if (!response) return route.abort("failed");
      if (!response.ok() || !/\bimmutable\b/.test(response.headers()["cache-control"] ?? "")) return route.fulfill({ response });
      const file = { status: response.status(), headers: response.headers(), body: await response.body() };
      kept.set(request.url(), file);
      return route.fulfill(file);
    },
  );
}

/** The two demo people of the Load journey: one proposes, the other approves (CC-34). */
// The two approvers hold the realm role `mfa-required` (deployment T-5120): their sign-in asks for a
// TOTP code, computed from the seed in PORTAL_TOTP / APPROVER_TOTP (e2e/live/totp.ts).
export const STEWARD = { user: "demo.steward@hel.fi", password: process.env.PORTAL_PASSWORD ?? "", totp: process.env.PORTAL_TOTP ?? "" };
export const APPROVER = { user: "demo.approver@hel.fi", password: process.env.APPROVER_PASSWORD ?? "", totp: process.env.APPROVER_TOTP ?? "" };
/** A person who may read the projects and change nothing. */
export const VIEWER = { user: "demo.viewer@hel.fi", password: process.env.VIEWER_PASSWORD ?? "" };
/**
 * The person who proposes and decides nothing (T-2231): bound to `editor` alone, so their own
 * change is the plain CC-34 refusal rather than the PF-58 administrator exception the steward
 * gets, and a role they hand out may carry no verb they lack (PF-50).
 */
export const EDITOR = { user: "demo.editor@hel.fi", password: process.env.EDITOR_PASSWORD ?? "" };
/**
 * The residue sweep's approver (T-2627): bound to `janitor`, which approves and deletes only
 * journey- and take-named resources of helsinki, so the sweep needs no demo person's delete.
 */
export const JANITOR = { user: "demo.janitor@hel.fi", password: process.env.JANITOR_PASSWORD ?? "" };
/**
 * The seeded workload on the Portal's side (T-2245, PF-45, PF-49): ServiceAccount
 * `helsinki/pipeline-proposer`, whose Keycloak client `helsinki-pipeline-proposer` mints a
 * `portal-api` token and holds `pipeline-editor` (propose DataSource and Pipeline) without approve
 * (PF-58). Its secret is Secret `keycloak-client-helsinki-pipeline-proposer`, key `client-secret`.
 */
export const PROPOSER = { client: "helsinki-pipeline-proposer", secret: process.env.PROPOSER_CLIENT_SECRET ?? "" };

/**
 * A `client_credentials` token of the proposer account, found the way an MCP client finds it: the
 * Portal's protected-resource metadata names the authorization server (RFC 9728), whose discovery
 * document names the token endpoint. Plain `fetch`, never the Playwright request context: the
 * journey header of the live config is refused beside a bearer token.
 */
export async function serviceAccountToken(baseURL: string): Promise<string> {
  if (!PROPOSER.secret) {
    throw new Error(`no PROPOSER_CLIENT_SECRET in the environment for ${PROPOSER.client}`);
  }
  const metadata = await fetch(new URL("/.well-known/oauth-protected-resource/api/v1/mcp", baseURL));
  expect(metadata.ok, "the Portal's protected-resource metadata").toBe(true);
  const issuer = ((await metadata.json()) as { authorization_servers?: string[] }).authorization_servers?.[0];
  if (!issuer) {
    throw new Error("the protected-resource metadata names no authorization server");
  }
  const discovery = await fetch(`${issuer.replace(/\/$/, "")}/.well-known/openid-configuration`);
  expect(discovery.ok, `the discovery document of ${issuer}`).toBe(true);
  const { token_endpoint: tokenEndpoint } = (await discovery.json()) as { token_endpoint: string };
  const answer = await fetch(tokenEndpoint, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "client_credentials",
      client_id: PROPOSER.client,
      client_secret: PROPOSER.secret,
    }),
  });
  if (!answer.ok) {
    // Keycloak's error names the cause (unauthorized_client, invalid_client); the secret is not in it.
    throw new Error(`the token endpoint refused ${PROPOSER.client}: ${answer.status} ${await answer.text()}`);
  }
  return ((await answer.json()) as { access_token: string }).access_token;
}

/**
 * The App probe (T-2795, AP-136): a member of every App's default group who reads `App` and
 * nothing else, so what it sees is what an App's own people see.
 */
export const PROBE = { user: "demo.probe@hel.fi", password: process.env.PROBE_PASSWORD ?? "" };
/**
 * The App sharing matrix's people (deployment seed/sharing, T-4911): the author holds `app-editor`
 * in project `sharing` and nothing else, the member reads `sharing`, the neighbour reads only
 * `sharing-other`. None holds a realm role, so none signs in with a TOTP code.
 */
export const AUTHOR = { user: "demo.author@hel.fi", password: process.env.AUTHOR_PASSWORD ?? "" };
export const MEMBER = { user: "demo.member@hel.fi", password: process.env.MEMBER_PASSWORD ?? "" };
/**
 * Organization members with no project and no RoleBinding (ADR-N-067 2.2, 2.7; T-6408): the first
 * in /helsinki/members alone, the second also in /helsinki/traffic-analysts. No realm role, no TOTP.
 */
export const ORGMEMBER = { user: "demo.orgmember@hel.fi", password: process.env.ORGMEMBER_PASSWORD ?? "" };
export const ANALYST = { user: "demo.analyst@hel.fi", password: process.env.ANALYST_PASSWORD ?? "" };
export const NEIGHBOUR = { user: "demo.neighbour@hel.fi", password: process.env.NEIGHBOUR_PASSWORD ?? "" };

/**
 * Signs one browser context in through the edge: the Portal's /login button, Keycloak's form
 * (`#username`, `#password`, `#kc-login`), back to the page asked for. Mirrors
 * joinedcontext-presentation/record/acts/_portal.py so a spec and a recording take one path.
 */
export async function signIn(
  browser: Browser,
  who: { user: string; password: string; totp?: string },
  path: string,
  ready: (page: Page) => Locator = portalReady,
  options: BrowserContextOptions = {},
): Promise<{ context: BrowserContext; page: Page }> {
  if (!who.password) {
    throw new Error(
      `no password in the environment for ${who.user} ` +
        "(PORTAL_PASSWORD / APPROVER_PASSWORD / VIEWER_PASSWORD / EDITOR_PASSWORD / JANITOR_PASSWORD / PROBE_PASSWORD / AUTHOR_PASSWORD / MEMBER_PASSWORD / ORGMEMBER_PASSWORD / ANALYST_PASSWORD / NEIGHBOUR_PASSWORD)",
    );
  }
  const context = await browser.newContext(options);
  await withLocalUi(context);
  await shareEdge(context);
  // A crash while signing in is signed in again on a fresh page of the same context (T-6011).
  const page = await openedTwice(context, (page) => goSignedIn(page, who, path, ready), `${who.user} signed in`);
  return { context, page };
}

/**
 * `WALK_UI_DIST=dist`: the browser runs this tree's built UI against the deployed Portal (T-3848),
 * so a walk judges a fix before it is deployed. Same origin, the same sign-in and the same API:
 * only the shell the Portal answers a page with and its `/assets/` come from the local build.
 */
export async function withLocalUi(context: BrowserContext): Promise<void> {
  const dist = process.env.WALK_UI_DIST;
  if (!dist) return;
  const shell = readFileSync(join(dist, "index.html"), "utf8");
  const portal = new URL(process.env.PORTAL_URL ?? "https://portal.dev.joinedcontext.com").host;
  await context.route((url) => url.host === portal && url.pathname.startsWith("/assets/"), async (route) => {
    const file = join(dist, new URL(route.request().url()).pathname);
    if (!existsSync(file)) return route.continue();
    const type = file.endsWith(".js") ? "text/javascript" : file.endsWith(".css") ? "text/css" : undefined;
    return route.fulfill({ body: readFileSync(file), contentType: type });
  });
  await context.route(
    (url) => url.host === portal && !/^\/(api|auth|assets)\//.test(url.pathname),
    async (route) => {
      if (route.request().resourceType() !== "document") return route.continue();
      const response = await route.fetch({ maxRedirects: 0 });
      const html = (response.headers()["content-type"] ?? "").includes("text/html") ? await response.text() : "";
      if (response.status() !== 200 || !html.includes("/assets/index-")) return route.fulfill({ response });
      return route.fulfill({ response, body: shell });
    },
  );
}

/** The Portal's own landmark: what every Portal page has once the sign-in is over. */
export const portalReady = (page: Page): Locator => page.getByRole("navigation", { name: "Main navigation" });

/**
 * Opens `url` and walks whatever sign-in stands in the way — the Portal's /login button, Keycloak's
 * form — until `ready` is visible. An application page has no Portal navigation, so a journey into
 * one names its own landmark (the app's heading) rather than waiting for one that never comes
 * (T-2309).
 */
export async function goSignedIn(
  page: Page,
  who: { user: string; password: string; totp?: string },
  url: string,
  ready: (page: Page) => Locator = portalReady,
): Promise<void> {
  // `load`, never `networkidle`: the Portal holds an activity stream and a drafts stream open
  // (`src/api/activity.ts`, `src/api/drafts.ts`, both `EventSource`) and polls a pending list
  // every ten seconds, so the network is never idle on any signed-in page. Waiting for it spent
  // the whole test budget on the login hop and every journey read as a timeout on whatever came
  // next (T-2452). Each step below waits for the thing it actually needs instead.
  // An anonymous `/` is the public catalogue (T-5627), which never bounces to /login: the sign-in
  // starts at /login, which returns to `/` once signed in, where a person keeps their home (T-6192).
  const home = url.startsWith("/") && new URL(url, "http://portal").pathname === "/";
  const refused: string[] = [];
  const onResponse = (response: Response) => {
    if (response.status() >= 400) refused.push(`${response.status()} ${new URL(response.url()).pathname}`);
  };
  page.on("response", onResponse);
  await page.goto(home ? `/login?redirect_to=${encodeURIComponent(url)}` : url, { waitUntil: "load" });
  let redirected = false;
  for (let step = 0; step < 4; step += 1) {
    if (await page.locator("#username").count()) {
      redirected = true;
      await page.fill("#username", who.user);
      await page.fill("#password", who.password);
      await submitted(page, () => page.click("#kc-login"));
      continue;
    }
    if (await answerOtp(page, who)) continue;
    const signInButton = page.getByRole("button", { name: "Sign in" });
    if (page.url().includes("/login") && (await signInButton.count())) {
      redirected = true;
      await submitted(page, () => signInButton.first().click());
      continue;
    }
    break;
  }
  // Playwright routes only the first request of a redirect chain, so the page the sign-in lands on
  // is the deployed shell, not `WALK_UI_DIST`'s: one reload takes the local build (T-5066).
  if (redirected && process.env.WALK_UI_DIST) {
    await page.reload({ waitUntil: "load" });
  }
  // The shell's own notice that its entry script never ran (index.html #boot-failed, opaque after
  // 8 s): the edge answered a chunk 429 (300 requests a minute per address, about 60 chunks a
  // first load, one egress address for the swarm and the walk's two people signing in together)
  // or a rollout swapped the assets mid-load. A reload inside the same minute meets the same 429,
  // so it waits the edge's window out first; the walk never reloaded and lost the viewer's whole
  // walk (T-5068). Twice at most: a Portal that never boots still fails below.
  for (let reload = 0; reload < 2 && (await bootFailed(page, ready)); reload += 1) {
    await page.waitForTimeout(BOOT_RETRY.waitMs);
    await page.reload({ waitUntil: "load" });
  }
  try {
    await expect(ready(page)).toBeVisible({ timeout: 60_000 });
  } catch (error) {
    throw new Error(`${await stoppedAt(page, refused)}: ${String(error)}`, { cause: error });
  } finally {
    page.off("response", onResponse);
  }
}

/**
 * Where a sign-in stood when the Portal never came ready (T-6510): the walker runs without a trace,
 * and the task filed from "expect(locator).toBeVisible() failed" could not tell Keycloak, an edge
 * refusal or a Portal error page apart. The address carries no query: Keycloak's holds its state.
 */
export async function stoppedAt(page: Page, refused: string[]): Promise<string> {
  const { origin, pathname } = new URL(page.url());
  const title = await page.title().catch(() => "");
  const text = await page.locator("body").innerText({ timeout: 2_000 }).catch(() => "");
  const answers = [...new Set(refused)].slice(0, 5).join(", ");
  return `sign-in stopped on ${origin}${pathname} (title "${title}", text "${text.replace(/\s+/g, " ").trim().slice(0, 100)}", ${refused.length} answers >= 400${answers ? `: ${answers}` : ""})`;
}

/**
 * Opens a page for the walker to judge, true when the edge still refuses it with 429 after two
 * loads again (T-5078). Otherwise it waits for the page a person looks at: the first heading, the
 * lists that read after it, the permissions document its write controls follow (T-5834), and
 * while index.html's fallback is the document (a script the edge refused), a load again after
 * `retryMs`, twice at most (T-5069).
 *
 * A load the edge refused anywhere, the document or one of its chunks, waits the edge's window out
 * and loads again, twice at most: the Portal reloads itself once for a refused chunk and that
 * reload met the same 429, so the walker judged the edge's own page and the Portal's console line
 * about the chunk (T-5743). `fresh` runs before each load again, so the caller drops what it
 * gathered from the refused load.
 */
export async function openToJudge(page: Page, address: string, retryMs = 20_000, fresh: () => void = () => undefined): Promise<boolean> {
  let refused: Response | undefined;
  const watch = (response: Response) => {
    if (response.status() === 429 && ["document", "script", "stylesheet"].includes(response.request().resourceType())) refused ??= response;
  };
  // Until permissions/me answers, PermissionGuard leaves a write control as it is (PF-51): a page
  // judged before that answer showed the viewer pressable writes the answer refuses (T-5834).
  const asking = new Set<Request>();
  const ask = (request: Request) => {
    if (request.url().endsWith("/permissions/me")) asking.add(request);
  };
  const answered = (request: Request) => asking.delete(request);
  page.on("response", watch);
  page.on("request", ask);
  page.on("requestfinished", answered);
  page.on("requestfailed", answered);
  try {
    let answer = await page.goto(address, { waitUntil: "load" });
    for (let retry = 0; ; retry++) {
      if (answer?.status() !== 429) {
        await settle(page, retryMs);
        await page.waitForTimeout(1500);
        await afterSelfReload(page, retryMs);
        for (const deadline = Date.now() + 30_000; asking.size > 0 && Date.now() < deadline; ) await page.waitForTimeout(200);
      }
      if (!refused) return false;
      if (retry === 2) return true;
      await page.waitForTimeout(edgeRetryMs(refused.headers()["retry-after"]));
      refused = undefined;
      fresh();
      answer = await page.reload({ waitUntil: "load" });
    }
  } finally {
    page.off("response", watch);
    page.off("request", ask);
    page.off("requestfinished", answered);
    page.off("requestfailed", answered);
  }
}

/** What the walker opened at an address: the edge's refusal, or the document and its one finding. */
export interface Opened {
  edgeRefused: boolean;
  /** The tab still shows a blank shell after the load again: no page to judge. */
  blank: boolean;
  /** The `document` finding, when the first load was not a page (T-5857). */
  document: string[];
}

/** The page document's answer as the walker saw it: status, body size and time to the answer. */
interface DocumentAnswer {
  status: number;
  bytes: number;
  ms: number;
}

function describeAnswer(answer: DocumentAnswer | undefined): string {
  return answer ? `${answer.status}, ${answer.bytes} B in ${answer.ms} ms` : "no answer";
}

/**
 * `openToJudge` for the walker, keeping the page document's last answer. A document answered
 * non-2xx, or a blank shell (`blankDocument`), is loaded once again after `againMs`: one load in a
 * rollout window made three a11y findings and three tasks on /organization/roles (T-5853..T-5855),
 * and the walker kept no status to name its cause. Either way it is one `document` finding.
 * A load where two /api reads or more answered 5xx (the Portal, not one route, did not answer) is
 * loaded once again too: fine then, it is one `api` finding instead of an `http` finding per read
 * (nine tasks for one load on /projects/helsinki/settings/roles/new, T-5937); still 5xx, the `http`
 * findings of the second load stand. A read that answered beside them does not keep the load from
 * that: /api/v1/branding answered (the browser holds it 5 min) while /auth/me and /branding/logo
 * answered 502 in a rollout, and the walker filed a task per read (T-6012, T-6013).
 */
export async function openToWalk(
  page: Page,
  address: string,
  retryMs = 20_000,
  fresh: () => void = () => undefined,
  againMs = 5_000,
): Promise<Opened> {
  let last: Response | undefined;
  let apis: number[] = [];
  const watch = (response: Response) => {
    const request = response.request();
    if (request.isNavigationRequest() && request.frame() === page.mainFrame()) last = response;
    if (new URL(response.url()).pathname.startsWith("/api/")) apis.push(response.status());
  };
  const failed = () => apis.filter((status) => status >= 500);
  const unreachable = () => failed().length > 1;
  const answer = async (): Promise<DocumentAnswer | undefined> => {
    if (!last) return undefined;
    const body = await last.body().catch(() => Buffer.alloc(0));
    return { status: last.status(), bytes: body.length, ms: Math.max(0, Math.round(last.request().timing().responseStart)) };
  };
  const bad = async (seen: DocumentAnswer | undefined) =>
    !seen || seen.status < 200 || seen.status > 299 || (await page.evaluate(blankDocument));
  page.on("response", watch);
  try {
    if (await openToJudge(page, address, retryMs, fresh)) return { edgeRefused: true, blank: false, document: [] };
    const first = await answer();
    const documentBad = await bad(first);
    if (!documentBad && !unreachable()) return { edgeRefused: false, blank: false, document: [] };
    const said = documentBad
      ? `document: the page answered ${describeAnswer(first)}`
      : `api: ${failed().length === apis.length ? "all" : `${failed().length} of`} ${apis.length} /api reads answered ${[...new Set(failed())].join(", ")}`;
    await page.waitForTimeout(againMs);
    fresh();
    last = undefined;
    apis = [];
    if (await openToJudge(page, address, retryMs, fresh)) return { edgeRefused: true, blank: false, document: [] };
    const second = await answer();
    const secondBad = await bad(second);
    if (!documentBad && !secondBad) return { edgeRefused: false, blank: false, document: failed().length > 0 ? [] : [`${said}; fine on retry`] };
    if (secondBad) {
      return { edgeRefused: false, blank: true, document: [`${said}, and ${describeAnswer(second)} on the load again ${againMs / 1000} s later`] };
    }
    return { edgeRefused: false, blank: false, document: [`${said}; blank on first load, fine on retry`] };
  } finally {
    page.off("response", watch);
  }
}

/**
 * What the walker finds on a page it opened with `openToWalk`: the edge's refusal, or the document
 * finding, then the page checks, axe's serious and critical violations and, for a viewer, the write
 * controls (UI-44). A blank shell skips the checks: they judge an empty document (T-5857).
 */
export async function judgeOpened(page: Page, opened: Opened, options: { namespaces: string[]; viewer: boolean }): Promise<string[]> {
  if (opened.edgeRefused) return ["boot: the edge refused the page with 429 Too Many Requests, after two loads again"];
  if (opened.blank) return opened.document;
  return [
    ...opened.document,
    ...(await page.evaluate(pageFindings, { namespaces: options.namespaces, layout: true })),
    ...(await axeViolations(page)).filter((violation) => /\((serious|critical)\)/.test(violation)).map((violation) => `axe: ${violation.slice(0, 200)}`),
    ...(options.viewer ? await page.evaluate(viewerWriteFindings, true) : []),
  ];
}

/**
 * The Portal reloads itself once for a chunk it could not load (T-4949): a check that lands in that
 * reload judged index.html's fallback of a page that drew a moment later (T-5185). Waits it out.
 */
export async function afterSelfReload(page: Page, retryMs = 20_000): Promise<void> {
  if ((await page.locator("#boot-failed").count()) === 0) return;
  await settle(page, retryMs);
  await page.waitForTimeout(1500);
}

async function settle(page: Page, retryMs: number): Promise<void> {
  const heading = () => page.locator("h1:visible").first().waitFor({ timeout: 30_000 }).catch(() => undefined);
  await heading();
  for (let retry = 0; retry < 2 && (await page.locator("#boot-failed").count()) > 0; retry++) {
    await page.waitForTimeout(retryMs);
    await page.reload({ waitUntil: "load" });
    await heading();
  }
}

/** How long a sign-in waits before it reloads a Portal that did not boot: the edge's rate window. */
export const BOOT_RETRY = { waitMs: 60_000 };

/** Waits up to 60 s for `ready` or the shell's boot notice; true when the notice showed. */
async function bootFailed(page: Page, ready: (page: Page) => Locator): Promise<boolean> {
  const notice = page.locator("#boot-failed");
  for (const deadline = Date.now() + 60_000; Date.now() < deadline; await page.waitForTimeout(500)) {
    if (await ready(page).isVisible()) return false;
    // Playwright counts opacity 0 as visible, so read the notice's own opacity.
    const shown = await notice.evaluate((element) => getComputedStyle(element).opacity === "1").catch(() => false);
    if (shown) return true;
  }
  return false;
}

/** Asks in the docked assistant: its first composer, or the conversation's once one is running. */
export async function ask(page: Page, text: string): Promise<void> {
  const bubble = page.getByRole("button", { name: "Open the assistant" });
  if (await bubble.count()) {
    await bubble.first().click();
  }
  const composer = page
    .getByLabel(/^(Ask the assistant|Tell the assistant what to build or change…)$/)
    .first();
  await composer.fill(text);
  await composer.press("Enter");
}

/** The change a proposal answered with, read off the notice the page shows (UI-23). */
export async function proposedChange(page: Page): Promise<string> {
  // A proposer who may approve gets "Open the change" on a change approved as proposed (PF-58).
  const review = page.getByRole("link", { name: /^(Review it in Approvals|Open the change)$/ });
  try {
    await expect(review).toBeVisible({ timeout: 60_000 });
  } catch (err) {
    // What the page says instead of the notice: a verdict, a refusal, a dialog still open.
    const said = await page.locator("[data-testid=form-page], [role=dialog], [role=alert], [role=status]").allInnerTexts();
    throw new Error(`no proposal notice; the page says: ${JSON.stringify(said)}\n${String(err)}`);
  }
  const href = (await review.getAttribute("href")) ?? "";
  const id = href.split("/approvals/")[1]?.split(/[?#]/)[0];
  if (!id) {
    throw new Error(`the notice links nowhere useful: ${href}`);
  }
  return id;
}

/**
 * Approves one change as the approver. A Yellow lane needs no typed confirmation; a Red one
 * (a public Endpoint, CC-19) asks for the resource name, typed as a person would.
 */
export async function approve(page: Page, project: string, change: string, confirm?: string): Promise<void> {
  // A Yellow change can be merged before the approver gets to it (on dev, 2026-10-18: chg-0000002b
  // read "Yellow Merged" with Approve disabled); that is the outcome this helper waits for.
  const state = await page.request.get(`/api/v1/projects/${project}/changes/${change}`);
  const phase = state.ok() ? ((await state.json()) as { status?: { phase?: string } }).status?.phase : undefined;
  if (phase && ["Deploying", "Merged", "Applied"].includes(phase)) return;
  await page.goto(`/projects/${project}/approvals/${change}?lang=en`, { waitUntil: "load" });
  const button = page.getByRole("button", { name: "Approve", exact: true });
  // Only a Red lane asks for the name typed back (CC-19). A caller that knows the name passes it
  // and this types it when the page asks; a Yellow change has no such field, and waiting for one
  // would fail on a change that needed no confirmation.
  const input = page.locator("#confirm-resource-name");
  // The lane arrives with the change's plan, after the load: wait until the page has either
  // enabled Approve (Yellow) or rendered the name field (Red), then answer the field if it is there.
  await expect(button.and(page.locator(":enabled")).or(input).first()).toBeVisible({ timeout: 60_000 });
  if (confirm && (await input.count())) {
    await expect(input).toBeEnabled({ timeout: 60_000 });
    await input.pressSequentially(confirm, { delay: 60 });
  }
  await expect(button).toBeEnabled({ timeout: 60_000 });
  await button.click();
  await expect(page.getByText(/Deploying|Merged|Applied|Live/).first()).toBeVisible({ timeout: 90_000 });
}

/**
 * Rejects one change as the approver, so a proposal a spec made leaves dev as it was.
 *
 * Reject opens a dialog and its confirmation stays disabled until a reason is typed: the proposer
 * reads that reason on the change. A helper that only clicked Reject left the dialog open and the
 * change PendingApproval — which is how this journey left changes behind on dev (T-1597).
 */
export async function reject(
  page: Page,
  project: string,
  change: string,
  reason = "Rejected by a live journey: this change was proposed only to prove the form.",
): Promise<void> {
  await page.goto(`/projects/${project}/approvals/${change}?lang=en`, { waitUntil: "load" });
  const button = page.getByRole("button", { name: "Reject", exact: true });
  await expect(button).toBeEnabled({ timeout: 60_000 });
  await button.click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Why are you rejecting this?").fill(reason);
  await dialog.getByRole("button", { name: "Reject the change" }).click();
  await expect(page.getByText(/Rejected/).first()).toBeVisible({ timeout: 60_000 });
}

/**
 * Proposes the deletion of one resource the way a person does (a Red Change, CC-19): its row's
 * menu on the kind's list, Remove, the name typed back, Propose removal; returns the change's id.
 * The row offers no "Delete …" button: the removal is one item of "More actions for …"
 * (`ResourceRowActions`), and the old selector matched nothing (T-2729).
 */
export async function proposeDelete(page: Page, project: string, plural: string, name: string): Promise<string> {
  await page.goto(`/projects/${project}/${plural}?lang=en`);
  const row = page.locator("tr, li").filter({ hasText: name }).first();
  await row.getByRole("button", { name: /^More actions for / }).click();
  await page.getByRole("menuitem", { name: "Remove" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel(`Type ${name} to confirm`).fill(name);
  await dialog.getByRole("button", { name: "Propose removal" }).click();
  const id = dialog.getByText(/^chg-[0-9a-f]{8}$/);
  await expect(id, `delete ${plural}/${name}`).toBeVisible({ timeout: 30_000 });
  return (await id.textContent()) ?? "";
}

/**
 * Deletes every draft of this journey and proves none is left (AG-61, T-2249).
 *
 * A journey that opens forms leaves drafts behind: the Portal keeps what a form holds so the person
 * can come back to it. Left on dev they are not harmless — 44 of them answered 2.3 MB on the drafts
 * route and the docked assistant could then answer nothing at all (T-2248). A deletion needs the
 * CSRF header like every other write, and the check afterwards is the point: without it the sweep
 * answers 403 and cleans nothing, which is what the first run of `hostile-names` did (T-1591).
 */
export async function sweepDrafts(
  context: BrowserContext,
  page: Page,
  project: string,
  mine: RegExp,
  also: { kind: string; name: string }[] = [],
): Promise<void> {
  const token = await csrf(context);
  const named = (draft: { name?: string; metadata?: { name?: string } }) =>
    draft.name ?? draft.metadata?.name ?? "";
  const left = new Map<string, { kind: string; name: string }>();
  const listed = await page.request.get(`/api/v1/projects/${project}/drafts`);
  if (listed.ok()) {
    for (const draft of ((await listed.json()).items ?? []) as {
      kind?: string;
      name?: string;
      metadata?: { name?: string };
    }[]) {
      const name = named(draft);
      if (mine.test(name)) {
        left.set(`${draft.kind ?? ""}/${name}`, { kind: draft.kind ?? "ContextSpace", name });
      }
    }
  }
  for (const one of also) {
    left.set(`${one.kind}/${one.name}`, one);
  }
  for (const { kind, name } of left.values()) {
    await page.request.delete(
      `/api/v1/projects/${project}/drafts/${kind}/${encodeURIComponent(name)}`,
      { headers: { "x-csrf-token": token } },
    );
  }
  const after = await page.request.get(`/api/v1/projects/${project}/drafts`);
  if (!after.ok()) {
    return;
  }
  const remaining = ((await after.json()).items ?? [])
    .map(named)
    .filter((name: string) => mine.test(name));
  expect(remaining, "a draft of this journey is still in the project").toEqual([]);
}

/** The names of a project's resources of one kind, as the list route answers them. */
export async function listedNames(page: Page, project: string, plural: string): Promise<string[]> {
  const answer = await page.request.get(`/api/v1/projects/${project}/${plural}`);
  expect(answer.ok(), `list ${plural}`).toBe(true);
  const body = (await answer.json()) as { items?: { metadata: { name: string } }[] };
  return (body.items ?? []).map((item) => item.metadata.name);
}

/**
 * The CSRF token of a signed-in context, so a spec's own API call goes through the same door the
 * page's calls go through. A request without it is refused by the middleware, and a spec that
 * reads that refusal as the rule it meant to test proves nothing (T-1585).
 */
export async function csrf(context: BrowserContext): Promise<string> {
  const cookie = (await context.cookies()).find((each) => each.name === "__Host-jc_csrf");
  if (!cookie) {
    throw new Error("no __Host-jc_csrf cookie in this context: the session did not complete");
  }
  return cookie.value;
}

/**
 * The check a manifest proposed without a draft needs first (PF-57, T-0956): the verdict gate
 * refuses an unchecked one on every door with `verdict_required`. The check records its verdict
 * under the manifest's own kind and name, fresh for these exact bytes; a proposal that became a
 * Change forgets it, so each door checks again (T-2631).
 */
export async function checkManifest(page: Page, context: BrowserContext, project: string, manifest: { kind: string } & Record<string, unknown>): Promise<void> {
  const [op, field] =
    manifest.kind === "Pipeline"
      ? ["jc_pipeline_test", "pipeline"]
      : manifest.kind === "DataSource"
        ? ["jc_datasource_check", "manifest"]
        : ["jc_manifest_dry_run", "manifest"];
  const answer = await page.request.post(`/api/v1/projects/${project}/ops/${op}`, {
    headers: { "x-csrf-token": await csrf(context), "content-type": "application/json" },
    data: { [field]: manifest },
  });
  const text = await answer.text();
  if (!answer.ok()) {
    throw new Error(`${op} of ${manifest.kind}: ${answer.status()} ${text}`);
  }
  const verdict = (JSON.parse(text) as { verdict?: { ok?: boolean } }).verdict;
  if (verdict?.ok !== true) {
    throw new Error(`${op} of ${manifest.kind} is not green: ${text.slice(0, 500)}`);
  }
}

/**
 * Removes a resource a journey created, all the way: the removal is itself a Change, so it is
 * proposed and then approved. A journey that only sent the DELETE left the resource standing and the
 * change open — which is how three spaces were found on dev on 2026-09-18 (T-2236).
 *
 * The **owner** approves it, not the approver: approving a removal needs `delete` on the kind
 * (`an_approver_approves_a_grant_only_within_their_own_rights_and_a_removal_only_with_delete`), which
 * the `approver` role does not carry and `demo.steward` does through `org-admin`. A steward
 * approving their own removal is the administrator exception of CC-34, the one
 * `roles-refusals.spec.ts` plays.
 */
export async function removeCompletely(
  owner: { context: BrowserContext; page: Page },
  project: string,
  plural: string,
  name: string,
): Promise<void> {
  const token = await csrf(owner.context);
  const answer = await owner.page.request.delete(`/api/v1/projects/${project}/${plural}/${name}`, {
    headers: { "x-csrf-token": token },
    data: { confirm: name },
  });
  if (answer.status() === 404) {
    return;
  }
  if (answer.status() !== 202) {
    throw new Error(`removal of ${plural}/${name} was refused: ${await answer.text()}`);
  }
  const change = ((await answer.json()) as { metadata?: { name?: string } }).metadata?.name ?? "";
  if (!change) {
    throw new Error(`the removal of ${plural}/${name} named no change`);
  }
  await approve(owner.page, project, change, name);
}

/**
 * Rejects every open Change of `name` through the approver door, with a reason naming the journey
 * (T-6081). A journey whose approval failed left its proposal pending, and `removeCompletely` read
 * the never-created resource as 404 and returned: 23 such proposals waited in helsinki's Approvals.
 * Rejecting closes the merge request; nothing is merged and no forge history is rewritten.
 */
export async function rejectOpen(
  owner: { context: BrowserContext; page: Page },
  project: string,
  name: string,
  journey: string,
): Promise<string[]> {
  const listed = await owner.page.request.get(`/api/v1/projects/${project}/changes`);
  if (!listed.ok()) {
    throw new Error(`list changes of ${project}: ${listed.status()} ${await listed.text()}`);
  }
  const mine = (((await listed.json()) as { items?: ListedChange[] }).items ?? []).filter(
    (change) => change.status.phase === "PendingApproval" && changeSubject(change) === name,
  );
  const token = await csrf(owner.context);
  for (const change of mine) {
    const answer = await owner.page.request.post(`/api/v1/projects/${project}/changes/${change.metadata.name}/reject`, {
      headers: { "x-csrf-token": token, "content-type": "application/json" },
      data: { reason: `Made by the live journey ${journey}, which cleans up after itself; never meant to be merged.` },
    });
    if (answer.status() !== 202) {
      throw new Error(`reject ${change.metadata.name} (${name}): ${answer.status()} ${await answer.text()}`);
    }
  }
  return mine.map((change) => change.metadata.name);
}

/**
 * A journey's `afterAll`: rejects what it left proposed, removes what it created, and rejects a
 * removal that could not be approved, so dev ends with neither the resource nor an open Change of
 * it, also when the journey failed half way (T-6081).
 */
export async function cleanUp(
  owner: { context: BrowserContext; page: Page },
  project: string,
  plural: string,
  name: string,
  journey: string,
): Promise<void> {
  await rejectOpen(owner, project, name, journey);
  try {
    await removeCompletely(owner, project, plural, name);
  } finally {
    await rejectOpen(owner, project, name, journey);
  }
}

/**
 * Waits until an approved removal has taken effect: the resource answers 404. A DataSource is
 * refused with 409 while a Pipeline that reads it is still there, so its Pipelines go first and
 * are waited out (T-5350).
 */
export async function waitGone(page: Page, project: string, plural: string, name: string): Promise<void> {
  await expect
    .poll(async () => (await page.request.get(`/api/v1/projects/${project}/${plural}/${name}`)).status(), {
      message: `${plural}/${name} is still there after its removal was approved`,
      timeout: 180_000,
      intervals: [5_000],
    })
    .toBe(404);
}

