import { readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { judgeOpened, openToJudge, openToWalk } from "./live/portal";

// T-5185: the walker judged /projects/helsinki/policies/new as "boot: the Portal did not load". The
// page drew its heading, then the Portal reloaded itself for a chunk it could not load (T-4949), and
// the check ran inside that reload, on index.html's fallback. Here the first document draws and
// reloads itself; the second is the real shell, drawn by its script a moment later.
test("the walker judges the page a Portal draws after reloading itself", async ({ page }) => {
  test.setTimeout(60_000);
  const shell = readFileSync("index.html", "utf8").replace(
    '<script type="module" src="/src/main.tsx"></script>',
    '<script>setTimeout(() => { document.getElementById("root").innerHTML = "<main><h1>New policy</h1></main>"; }, 3000);</script>',
  );
  let documents = 0;
  await page.route("http://portal.test/**", (route) => {
    documents += 1;
    const body = documents === 1 ? "<main><h1>New policy</h1></main><script>setTimeout(() => location.reload(), 500);</script>" : shell;
    return route.fulfill({ body, contentType: "text/html" });
  });
  expect(await openToJudge(page, "http://portal.test/projects/helsinki/policies/new", 1_000)).toBe(false);
  // Read once, as the walker's checks do: an expect that retries would wait out the reload itself.
  expect(await page.locator("#boot-failed").count()).toBe(0);
  expect(await page.getByRole("heading", { level: 1, name: "New policy" }).isVisible()).toBe(true);
  expect(documents).toBe(2);
});

// T-5743: the walker reported "console: TypeError: Failed to fetch dynamically imported module" on /
// with the edge's own "Too many requests" page in the shot. The edge refused the App chunk (429), the
// Portal logged it and reloaded itself, and that reload met the same 429. Here the first document's
// chunk is refused, its reload is refused, and the load after the edge's window draws the page.
test("the walker judges a page the edge refused only after the edge's window, without the refused load's console", async ({ page }) => {
  test.setTimeout(60_000);
  const logged: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error" && !message.text().startsWith("Failed to load resource")) logged.push(message.text());
  });
  const portal = '<main><h1>Home</h1></main><script>import("/assets/App.js").catch((error) => { console.error(String(error)); setTimeout(() => location.reload(), 200); });</script>';
  let documents = 0;
  let chunks = 0;
  await page.route("http://portal.test/**", (route) => {
    if (route.request().url().endsWith("/assets/App.js")) {
      chunks += 1;
      return chunks === 1
        ? route.fulfill({ status: 429, headers: { "retry-after": "0" }, body: "" })
        : route.fulfill({ body: "export default 1;", contentType: "text/javascript" });
    }
    documents += 1;
    return documents === 2
      ? route.fulfill({ status: 429, headers: { "retry-after": "0" }, body: "<main><h1>Too many requests</h1></main>", contentType: "text/html" })
      : route.fulfill({ body: portal, contentType: "text/html" });
  });
  expect(await openToJudge(page, "http://portal.test/", 1_000, () => logged.splice(0))).toBe(false);
  expect(await page.getByRole("heading", { level: 1, name: "Home" }).isVisible()).toBe(true);
  expect(logged).toEqual([]);
  expect(documents).toBe(3);
});

// A Portal the edge keeps refusing stays a finding: openToJudge says so after two loads again.
test("the walker reports a page the edge refuses on every load", async ({ page }) => {
  test.setTimeout(60_000);
  await page.route("http://portal.test/**", (route) =>
    route.fulfill({ status: 429, headers: { "retry-after": "0" }, body: "<main><h1>Too many requests</h1></main>", contentType: "text/html" }),
  );
  expect(await openToJudge(page, "http://portal.test/", 1_000)).toBe(true);
});

// T-5834: the walker judged "Add an assistant" on /projects/helsinki/knowledge as pressable by the
// viewer at 1440 and refused at 400. The heading drew, permissions/me answered after the walker's
// 1.5 s, and until then PermissionGuard leaves a control as it is (PF-51). Here the answer takes 3 s.
test("the walker judges a page's controls after its permissions document answered", async ({ page }) => {
  test.setTimeout(60_000);
  const portal =
    '<main><h1>Knowledge</h1><button id="add">Add an assistant</button></main><script>fetch("/api/v1/projects/helsinki/permissions/me").then(() => document.getElementById("add").setAttribute("aria-disabled", "true"));</script>';
  await page.route("http://portal.test/**", async (route) => {
    if (route.request().url().endsWith("/permissions/me")) {
      await new Promise((resolve) => setTimeout(resolve, 3_000));
      return route.fulfill({ body: '{"grants":[]}', contentType: "application/json" });
    }
    return route.fulfill({ body: portal, contentType: "text/html" });
  });
  expect(await openToJudge(page, "http://portal.test/projects/helsinki/knowledge", 1_000)).toBe(false);
  expect(await page.locator("#add").getAttribute("aria-disabled")).toBe("true");
});

// T-5857: one blank load of /organization/roles in a rollout window became three walker findings
// (one-h1, document-title, html-has-lang) and three tasks, with no status to name the cause. Now
// the walker keeps the document's answer, loads a blank or non-2xx page once more, and says so once.
const ROLES = '<!doctype html><html lang="en"><head><title>Roles</title></head><body><div id="root"><main><h1>Roles</h1><p>Who may do what in the organization.</p></main></div></body></html>';
const BAD_GATEWAY = "<html><body><h1>502 Bad Gateway</h1></body></html>";
const A11Y = /one-h1|document-title|html-has-lang/;

async function walkStub(page: Page, documents: string[]): Promise<{ findings: string[]; loads: number }> {
  let loads = 0;
  await page.route("http://portal.test/**", (route) => {
    const bad = documents[loads] === "502";
    loads += 1;
    return route.fulfill({ status: bad ? 502 : 200, body: bad ? BAD_GATEWAY : ROLES, contentType: "text/html" });
  });
  const opened = await openToWalk(page, "http://portal.test/organization/roles?lang=en", 1_000, undefined, 100);
  return { findings: await judgeOpened(page, opened, { namespaces: [], viewer: false }), loads };
}

test("the walker reports a page document answered 502 once as one document finding, fine on retry", async ({ page }) => {
  test.setTimeout(60_000);
  const { findings, loads } = await walkStub(page, ["502", "200"]);
  const documents = findings.filter((finding) => finding.startsWith("document:"));
  expect(documents).toHaveLength(1);
  expect(documents[0]).toMatch(/^document: the page answered 502, \d+ B in \d+ ms; blank on first load, fine on retry$/);
  expect(findings.filter((finding) => A11Y.test(finding))).toEqual([]);
  expect(loads).toBe(2);
});

test("the walker reports a page document answered 502 twice as one document finding, not fine on retry", async ({ page }) => {
  test.setTimeout(60_000);
  const { findings, loads } = await walkStub(page, ["502", "502"]);
  expect(findings).toHaveLength(1);
  expect(findings[0]).toMatch(/^document: the page answered 502, \d+ B in \d+ ms, and 502, \d+ B in \d+ ms on the load again/);
  expect(findings[0]).not.toContain("fine on retry");
  expect(loads).toBe(2);
});

test("the walker judges a normal page without a document finding", async ({ page }) => {
  test.setTimeout(60_000);
  const { findings, loads } = await walkStub(page, ["200"]);
  expect(findings.filter((finding) => finding.startsWith("document:"))).toEqual([]);
  expect(findings.filter((finding) => A11Y.test(finding))).toEqual([]);
  expect(loads).toBe(1);
});

// T-5937: in the walk after the 474a7f1 rollout, /projects/helsinki/settings/roles/new drew its
// shell while all nine of its /api reads answered 502, and the walker filed nine tasks (T-5937..
// T-5944) for one load. A load whose every /api read is 5xx is loaded once again, and fine then it
// is one `api` finding; still 5xx, the `http` findings of the load again stand.
const READS = `${ROLES.replace("</body>", "<script>fetch('/api/v1/branding');fetch('/api/v1/projects');</script></body>")}`;

async function walkReads(page: Page, reads: number[]): Promise<{ findings: string[]; loads: number }> {
  let loads = 0;
  let current: string[] = [];
  page.on("response", (response) => {
    if (response.url().includes("/api/") && response.status() >= 400) current.push(`http: ${response.status()} ${new URL(response.url()).pathname}`);
  });
  await page.route("http://portal.test/**", (route) => {
    if (!route.request().url().includes("/api/")) {
      loads += 1;
      return route.fulfill({ status: 200, body: READS, contentType: "text/html" });
    }
    return route.fulfill({ status: reads[loads - 1] ?? 200, body: "{}", contentType: "application/json" });
  });
  const opened = await openToWalk(page, "http://portal.test/projects/helsinki/settings/roles/new?lang=en", 1_000, () => {
    current = [];
  }, 100);
  return { findings: [...current, ...(await judgeOpened(page, opened, { namespaces: [], viewer: false }))], loads };
}

test("the walker reports a load whose every /api read answered 502 once as one api finding, fine on retry", async ({ page }) => {
  test.setTimeout(60_000);
  const { findings, loads } = await walkReads(page, [502, 200]);
  expect(findings.filter((finding) => finding.startsWith("http:"))).toEqual([]);
  expect(findings.filter((finding) => finding.startsWith("api:"))).toEqual(["api: all 2 /api reads answered 502; fine on retry"]);
  expect(loads).toBe(2);
});

test("the walker keeps the http findings of a load whose /api reads answered 502 twice", async ({ page }) => {
  test.setTimeout(60_000);
  const { findings, loads } = await walkReads(page, [502, 502]);
  expect(findings.filter((finding) => finding.startsWith("http:")).sort()).toEqual(["http: 502 /api/v1/branding", "http: 502 /api/v1/projects"]);
  expect(findings.filter((finding) => finding.startsWith("api:"))).toEqual([]);
  expect(loads).toBe(2);
});

test("the walker keeps the http finding of one route answering 502 beside reads that answered", async ({ page }) => {
  test.setTimeout(60_000);
  let loads = 0;
  const current: string[] = [];
  page.on("response", (response) => {
    if (response.url().includes("/api/") && response.status() >= 400) current.push(`http: ${response.status()} ${new URL(response.url()).pathname}`);
  });
  await page.route("http://portal.test/**", (route) => {
    if (!route.request().url().includes("/api/")) {
      loads += 1;
      return route.fulfill({ status: 200, body: READS, contentType: "text/html" });
    }
    const failing = route.request().url().endsWith("/branding");
    return route.fulfill({ status: failing ? 502 : 200, body: "{}", contentType: "application/json" });
  });
  const opened = await openToWalk(page, "http://portal.test/projects/helsinki/settings/roles/new?lang=en", 1_000, undefined, 100);
  expect(opened.document).toEqual([]);
  expect(current).toEqual(["http: 502 /api/v1/branding"]);
  expect(loads).toBe(1);
});

// T-6012: in the walk after the 25e2920 rollout, /projects/helsinki/import read /api/v1/branding
// (the browser holds it 5 min) while /auth/me and /branding/logo answered 502, so the load was not
// "every read 5xx" and the walker filed a task per read (T-6012, T-6013). Two 5xx reads are enough.
const MIXED = `${ROLES.replace(
  "</body>",
  "<script>fetch('/api/v1/branding');fetch('/api/v1/auth/me');fetch('/api/v1/branding/logo');</script></body>",
)}`;

async function walkMixed(page: Page, failing: number[]): Promise<{ findings: string[]; loads: number }> {
  let loads = 0;
  let current: string[] = [];
  page.on("response", (response) => {
    if (response.url().includes("/api/") && response.status() >= 400) current.push(`http: ${response.status()} ${new URL(response.url()).pathname}`);
  });
  await page.route("http://portal.test/**", (route) => {
    const url = route.request().url();
    if (!url.includes("/api/")) {
      loads += 1;
      return route.fulfill({ status: 200, body: MIXED, contentType: "text/html" });
    }
    const status = url.endsWith("/api/v1/branding") ? 200 : (failing[loads - 1] ?? 200);
    return route.fulfill({ status, body: "{}", contentType: "application/json" });
  });
  const opened = await openToWalk(page, "http://portal.test/projects/helsinki/import?lang=en", 1_000, () => {
    current = [];
  }, 100);
  return { findings: [...current, ...(await judgeOpened(page, opened, { namespaces: [], viewer: false }))], loads };
}

test("the walker reports a load whose two /api reads answered 502 beside one that answered as one api finding, fine on retry", async ({ page }) => {
  test.setTimeout(60_000);
  const { findings, loads } = await walkMixed(page, [502, 200]);
  expect(findings.filter((finding) => finding.startsWith("http:"))).toEqual([]);
  expect(findings.filter((finding) => finding.startsWith("api:"))).toEqual(["api: 2 of 3 /api reads answered 502; fine on retry"]);
  expect(loads).toBe(2);
});

test("the walker keeps the http findings of two /api reads answering 502 on the load again", async ({ page }) => {
  test.setTimeout(60_000);
  const { findings, loads } = await walkMixed(page, [502, 502]);
  expect(findings.filter((finding) => finding.startsWith("http:")).sort()).toEqual(["http: 502 /api/v1/auth/me", "http: 502 /api/v1/branding/logo"]);
  expect(findings.filter((finding) => finding.startsWith("api:"))).toEqual([]);
  expect(loads).toBe(2);
});
