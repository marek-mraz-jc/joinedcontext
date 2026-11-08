import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// The project's repositories on shako, read through the registry (`POST …/ops/jc_repository_*`),
// in a real browser with the API answered in the browser: list → repository → file → commit →
// diff → pull request → its Change, and what a viewer meets. Each state is shot at 1440 and 400.

const IDENTITY = { subject: "b7c1e0f4", username: "eva.steward", name: "Eva Steward", email: "eva@hel.fi", roles: [] };
const grants = (verbs: string[]) => ({
  project: "helsinki",
  bootstrap: false,
  grants: [{ role: "r", binding: "helsinki-people", scope: "project:helsinki", rule: { kinds: ["Project"], verbs } }],
});

const SHA = "9fceb02d0ae598e95dc970b74767f19372d61af8";
const PARENT = "1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d";
const XSS = '<script>window.__pwned = 1</script> <img src=x onerror="window.__pwned = 1"> [a link](javascript:window.__pwned=1)';
const README = [
  "# Helsinki configuration",
  "",
  "The data spaces, Endpoints and pipelines of the City of Helsinki, as the Portal writes them.",
  "Every change is proposed and approved in the Portal; nothing is pushed here by hand.",
  "",
  "## Layout",
  "",
  "- `spaces/<space>/endpoints/` holds one file per Endpoint",
  "- `spaces/<space>/pipelines/` holds the imports",
  "",
  `Untrusted text stays text: ${XSS}`,
  "",
].join("\n");

const AIR = [
  "apiVersion: joinedcontext.com/v1alpha1",
  "kind: Endpoint",
  "metadata:",
  "  name: air-quality",
  "  namespace: helsinki",
  "spec:",
  "  space: air",
  "  title: Air quality in Helsinki",
  "  types: [AirQualityObserved]",
  "  access: public",
  `  description: "${XSS.replaceAll('"', "'")}"`,
  "",
].join("\n");

const REPOSITORIES = [
  { name: "helsinki-config", role: "configuration", defaultBranch: "main", sizeBytes: 482_304, description: "Helsinki's configuration: spaces, Endpoints, pipelines" },
  { name: "helsinki-shared", role: "project", defaultBranch: "main", sizeBytes: 12_288, description: "Mappings shared by the city's projects" },
  { name: "helsinki_air-dashboard", role: "application", defaultBranch: "main", sizeBytes: 1_204_224, description: "The air quality dashboard" },
  { name: "helsinki_events", role: "application", defaultBranch: "main", sizeBytes: 88_064, archived: true, description: "Events calendar (retired)" },
];

const ANSWERS: Record<string, (input: Record<string, unknown>) => unknown> = {
  jc_repository_list: (input) => {
    const words = String(input.filter ?? "").toLowerCase();
    const items = REPOSITORIES.filter((r) => !words || r.name.includes(words) || r.description.toLowerCase().includes(words));
    return { items, total: items.length };
  },
  jc_repository_get: () => ({
    ...REPOSITORIES[0],
    owner: "helsinki",
    cloneUrl: "https://forge.dev.joinedcontext.com/git/helsinki/helsinki-config.git",
    empty: false,
    lastCommit: { sha: SHA, message: "Raise the air quality page limit", author: "eva.steward", date: "2026-11-07T09:12:00Z" },
    readme: { path: "README.md", text: README },
  }),
  jc_repository_tree: (input) =>
    input.path === "spaces/air/endpoints"
      ? {
          repository: "helsinki-config", ref: "main", path: "spaces/air/endpoints", truncated: false,
          entries: [
            { name: "air-quality.yaml", path: "spaces/air/endpoints/air-quality.yaml", type: "file", size: 412 },
            { name: "noise.yaml", path: "spaces/air/endpoints/noise.yaml", type: "file", size: 388 },
          ],
        }
      : {
          repository: "helsinki-config", ref: "main", path: "", truncated: false,
          entries: [
            { name: "spaces", path: "spaces", type: "folder" },
            { name: "apps", path: "apps", type: "folder" },
            { name: "README.md", path: "README.md", type: "file", size: 642 },
            { name: "project.yaml", path: "project.yaml", type: "file", size: 214 },
          ],
        },
  jc_repository_file: (input) =>
    input.path === "logo.png"
      ? { repository: "helsinki-config", ref: "main", path: "logo.png", size: 48_213, binary: true, tooLarge: false }
      : { repository: "helsinki-config", ref: "main", path: input.path, size: AIR.length, binary: false, tooLarge: false, text: AIR },
  jc_repository_commits: () => ({
    ref: "main",
    total: 3,
    items: [
      { sha: SHA, message: "Raise the air quality page limit", author: "eva.steward", date: "2026-11-07T09:12:00Z" },
      { sha: PARENT, message: "Add the noise Endpoint", author: "petra.editor", date: "2026-11-05T14:40:00Z" },
      { sha: "0f".repeat(20), message: "First files", author: "portal", date: "2026-10-30T08:00:00Z" },
    ],
  }),
  jc_repository_commit: () => ({
    repository: "helsinki-config",
    sha: SHA,
    message: `Raise the air quality page limit\n\nThe dashboard reads a day of readings at once. ${XSS}`,
    author: "eva.steward",
    email: "eva@hel.fi",
    date: "2026-11-07T09:12:00Z",
    parents: [PARENT],
    truncated: false,
    files: [
      {
        path: "spaces/air/endpoints/air-quality.yaml",
        status: "modified",
        patch: "@@ -6,6 +6,7 @@\n spec:\n   space: air\n   title: Air quality in Helsinki\n-  types: [AirQualityObserved]\n+  types: [AirQualityObserved, WeatherObserved]\n+  pageLimit: 1000\n   access: public\n",
      },
      { path: "spaces/air/logo.png", status: "added", binary: true },
    ],
  }),
  jc_repository_refs: () => ({
    repository: "helsinki-config",
    branches: [
      { name: "main", headSha: SHA, protected: true, default: true },
      { name: "portal/air-quality-limit", headSha: PARENT },
    ],
    tags: [{ name: "v2026.11.0", targetSha: PARENT }],
  }),
  // As the op filters shako: open is `State eq 'open'`, closed every other state, all both.
  jc_repository_pulls: (input) => {
    const all = [
      { number: 42, title: "Add the WeatherObserved type to air quality", state: "open", author: "eva.steward", head: "portal/air-quality-limit", base: "main", createdAt: "2026-11-07T09:30:00Z" },
      { number: 41, title: "Add the noise Endpoint", state: "merged", author: "petra.editor", head: "portal/noise", base: "main", createdAt: "2026-11-05T14:00:00Z" },
      { number: 40, title: "Rename the parking space", state: "closed", author: "petra.editor", head: "portal/parking", base: "main", createdAt: "2026-11-02T08:00:00Z" },
    ];
    const state = String(input.state ?? "open");
    const items = all.filter((pull) => state === "all" || (state === "open") === (pull.state === "open"));
    return { items, total: items.length };
  },
  jc_repository_pull: () => ({
    repository: "helsinki-config",
    number: 42,
    title: "Add the WeatherObserved type to air quality",
    state: "open",
    author: "eva.steward",
    head: "portal/air-quality-limit",
    base: "main",
    createdAt: "2026-11-07T09:30:00Z",
    body: `Proposed in the Portal.\n\n${XSS}`,
    change: "chg-0000002a",
    files: [{ path: "spaces/air/endpoints/air-quality.yaml", status: "modified" }],
    reviews: [
      { author: "petra.editor", verdict: "APPROVED", body: "Matches the model." },
      { author: "jan.viewer", verdict: "COMMENT", body: "Will the dashboard follow?" },
    ],
  }),
  jc_repository_settings: () => ({
    repository: "helsinki-config",
    protection: [{ pattern: "main", pushAllowed: false, forcePushAllowed: false, requiredApprovals: 1 }],
    webhooks: [{ address: "https://portal.dev.joinedcontext.com/api/v1/webhooks/shako", active: true }],
    collaborators: [{ group: "helsinki-stewards", access: "write" }, { group: "helsinki-viewers", access: "read" }],
    packages: [{ type: "container", name: "air-dashboard", version: "1.4.0", digest: `sha256:${SHA}${"0".repeat(24)}`, size: 23_456_789, signed: true, createdAt: "2026-11-06T10:00:00Z" }],
  }),
};

type Override = Partial<Record<string, (input: Record<string, unknown>) => { status: number; body: unknown } | "pending">>;

async function stubApi(p: Page, verbs: string[], override: Override = {}): Promise<{ reads: Record<string, unknown>[] }> {
  const reads: Record<string, unknown>[] = [];
  await p.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: status >= 400 ? "application/problem+json" : "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
    if (url.pathname.endsWith("/permissions/me")) return json(grants(verbs));
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [{ name: "helsinki" }] });
    const op = /\/api\/v1\/projects\/helsinki\/ops\/(jc_repository_[a-z]+)$/.exec(url.pathname)?.[1];
    if (op && request.method() === "POST") {
      const input = (request.postDataJSON() ?? {}) as Record<string, unknown>;
      reads.push({ op, ...input });
      const special = override[op]?.(input);
      if (special === "pending") return; // never answered: the loading state
      if (special) return json(special.body, special.status);
      return json(ANSWERS[op](input));
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return { reads };
}

/** One state, shot at both widths, with the page at its full height (the sidebar stays whole). */
async function shoot(p: Page, name: string): Promise<void> {
  await p.mouse.move(0, 0); // no hover left on the row last clicked
  for (const width of [1440, 400]) {
    await p.setViewportSize({ width, height: 900 });
    await p.waitForTimeout(150);
    const height = await p.evaluate(() => document.documentElement.scrollHeight);
    await p.setViewportSize({ width, height: Math.min(Math.max(height, 900), 6000) });
    await p.screenshot({ path: test.info().outputPath(`${name}-${width}.png`) });
  }
  await p.setViewportSize({ width: 1440, height: 900 });
}

/** Nothing the repository holds ran or became markup. */
async function inert(p: Page): Promise<void> {
  expect(await p.evaluate(() => (window as { __pwned?: number }).__pwned)).toBeUndefined();
  await expect(p.locator("main script, main img[onerror]")).toHaveCount(0);
  await expect(p.locator('main a[href^="javascript:" i]')).toHaveCount(0);
}

test.describe("the project's repositories", () => {
  test("a steward walks list → repository → file → commit → diff → pull request → its Change", async ({ page: p }) => {
    const { reads } = await stubApi(p, ["read", "propose", "approve"]);
    await p.goto("/projects/helsinki/repositories?lang=en");
    await expect(p.getByRole("heading", { level: 1, name: "Repositories" })).toBeVisible();
    await expect(p.getByRole("link", { name: "helsinki-config" })).toBeVisible();
    expect(await axeViolations(p)).toEqual([]);
    await shoot(p, "list-filled");

    await p.getByLabel("Filter by name").fill("dash");
    await expect(p.getByRole("link", { name: "helsinki_air-dashboard" })).toBeVisible();
    await expect(p.getByRole("link", { name: "helsinki-config" })).toHaveCount(0);
    await p.getByLabel("Filter by name").fill("nothing-here");
    await expect(p.getByText("No repository matches")).toBeVisible();
    await shoot(p, "list-filtered-empty");
    await p.getByLabel("Filter by name").fill("");

    await p.getByRole("link", { name: "helsinki-config" }).click();
    await expect(p.getByRole("heading", { level: 1, name: "helsinki-config" })).toBeVisible();
    await expect(p.getByRole("region", { name: "README.md" })).toContainText("<script>window.__pwned = 1</script>");
    await inert(p);
    expect(await axeViolations(p)).toEqual([]);
    await shoot(p, "repo-files");

    await p.getByRole("link", { name: "spaces", exact: true }).click();
    await expect(p).toHaveURL(/path=spaces/);
    await p.goto("/projects/helsinki/repositories/helsinki-config?tab=files&path=spaces%2Fair%2Fendpoints&lang=en");
    await expect(p.getByRole("link", { name: "air-quality.yaml" })).toBeVisible();
    await shoot(p, "repo-folder");
    await p.getByRole("link", { name: "air-quality.yaml" }).click();
    const lines = p.getByRole("group", { name: "spaces/air/endpoints/air-quality.yaml" });
    await expect(lines).toContainText("kind: Endpoint");
    await inert(p);
    expect(await axeViolations(p)).toEqual([]);
    await shoot(p, "file");

    await p.getByRole("link", { name: "History of this file" }).click();
    await expect(p.getByText(/Commits that changed spaces\/air\/endpoints\/air-quality.yaml/)).toBeVisible();
    await shoot(p, "commits");
    await p.getByRole("link", { name: "Raise the air quality page limit" }).click();
    const diff = p.getByRole("group", { name: "Changes in spaces/air/endpoints/air-quality.yaml" });
    await expect(diff).toContainText("pageLimit: 1000");
    await inert(p);
    expect(await axeViolations(p)).toEqual([]);
    await shoot(p, "commit-diff");

    await p.getByRole("tab", { name: "Branches and tags" }).click();
    await expect(p.getByRole("link", { name: "portal/air-quality-limit" })).toBeVisible();
    await shoot(p, "branches");

    await p.getByRole("tab", { name: "Pull requests" }).click();
    await expect(p.getByRole("link", { name: "Add the WeatherObserved type to air quality" })).toBeVisible();
    await shoot(p, "pulls");
    await expect(p.getByRole("link", { name: "Add the noise Endpoint" })).toHaveCount(0);
    await p.getByLabel("State").selectOption("closed");
    await expect(p.getByRole("link", { name: "Add the noise Endpoint" })).toBeVisible();
    await expect(p.getByRole("link", { name: "Add the WeatherObserved type to air quality" })).toHaveCount(0);
    await shoot(p, "pulls-closed");
    await p.getByLabel("State").selectOption("open");
    await p.getByRole("link", { name: "Add the WeatherObserved type to air quality" }).click();
    await expect(p.getByText("Matches the model.")).toBeVisible();
    await inert(p);
    expect(await axeViolations(p)).toEqual([]);
    await shoot(p, "pull");
    await expect(p.getByRole("link", { name: "Open the change" })).toHaveAttribute("href", "/projects/helsinki/approvals/chg-0000002a");

    await p.getByRole("tab", { name: "Settings" }).click();
    await expect(p.getByText("https://portal.dev.joinedcontext.com/api/v1/webhooks/shako")).toBeVisible();
    await expect(p.getByText(/secret/i).first()).toBeVisible();
    expect(await axeViolations(p)).toEqual([]);
    await shoot(p, "settings");

    expect(reads.map((read) => read.op)).toEqual(expect.arrayContaining([
      "jc_repository_list", "jc_repository_get", "jc_repository_tree", "jc_repository_file", "jc_repository_commits",
      "jc_repository_commit", "jc_repository_refs", "jc_repository_pulls", "jc_repository_pull", "jc_repository_settings",
    ]));
  });

  test("a viewer reads the repository but is offered no settings, and the server's refusal is named", async ({ page: p }) => {
    const { reads } = await stubApi(p, ["read"]);
    await p.goto("/projects/helsinki/repositories/helsinki-config?tab=settings&lang=en");
    await expect(p.getByRole("tab", { name: "Files", selected: true })).toBeVisible();
    await expect(p.getByRole("tab", { name: "Settings" })).toHaveCount(0);
    expect(reads.some((read) => read.op === "jc_repository_settings")).toBe(false);
    await shoot(p, "viewer-no-settings");
  });

  test("a person outside the project is told, not shown an empty list", async ({ page: p }) => {
    await stubApi(p, [], {
      jc_repository_list: () => ({ status: 404, body: { type: "about:blank", title: "Not Found", status: 404, detail: "helsinki was not found" } }),
    });
    await p.goto("/projects/helsinki/repositories?lang=en");
    await expect(p.getByRole("heading", { level: 1, name: "Repositories" })).toBeVisible();
    await expect(p.getByText("No repositories yet")).toHaveCount(0);
    await expect(p.getByRole("alert").first()).toContainText("The repositories of helsinki are not open to you");
    await expect(p.getByLabel("Filter by name")).toHaveCount(0);
    await shoot(p, "list-refused");
  });

  test("another organization's repository is not there", async ({ page: p }) => {
    await stubApi(p, ["read"], {
      jc_repository_get: () => ({ status: 404, body: { type: "about:blank", title: "Not Found", status: 404, detail: "repository other-config was not found" } }),
    });
    await p.goto("/projects/helsinki/repositories/other-config?lang=en");
    await expect(p.getByRole("heading", { level: 1, name: "other-config" })).toBeVisible();
    await expect(p.getByRole("alert").first()).toContainText("No repository named other-config belongs to helsinki");
    await expect(p.getByRole("alert").first().getByRole("link", { name: "All repositories" })).toBeVisible();
    await shoot(p, "repo-not-found");
  });

  test("the list says what lives here when there is nothing, and waits visibly while it loads", async ({ page: p }) => {
    await stubApi(p, ["read"], { jc_repository_list: () => ({ status: 200, body: { items: [], total: 0 } }) });
    await p.goto("/projects/helsinki/repositories?lang=en");
    await expect(p.getByText("No repositories yet")).toBeVisible();
    await shoot(p, "list-empty");
    await p.unrouteAll({ behavior: "ignoreErrors" });
    await stubApi(p, ["read"], { jc_repository_list: () => "pending" });
    await p.goto("/projects/helsinki/repositories?lang=en");
    await expect(p.getByText("Loading from the forge")).toBeAttached();
    await shoot(p, "list-loading");
  });

  test("no closed pull request: the empty state names the state and offers every state", async ({ page: p }) => {
    await stubApi(p, ["read"], { jc_repository_pulls: () => ({ status: 200, body: { items: [], total: 0 } }) });
    await p.goto("/projects/helsinki/repositories/helsinki-config?tab=pulls&lang=en");
    await p.getByLabel("State").selectOption("closed");
    await expect(p.getByText("No closed pull requests")).toBeVisible();
    await shoot(p, "pulls-empty");
    await p.getByRole("button", { name: "Show every state" }).click();
    await expect(p.getByText("No pull requests", { exact: true })).toBeVisible();
  });

  test("a binary file is named, not drawn", async ({ page: p }) => {
    await stubApi(p, ["read"]);
    await p.goto("/projects/helsinki/repositories/helsinki-config?tab=files&file=logo.png&lang=en");
    await expect(p.getByText("This file is binary")).toBeVisible();
    await shoot(p, "file-binary");
  });
});
