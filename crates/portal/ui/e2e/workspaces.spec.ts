/**
 * The workspace bar and the conflict chooser in a browser (T-1254, T-1255; UI-15, UI-61, UI-62).
 *
 * `tests/workspaces_a11y.test.tsx` reads their markup under jsdom, which has no layout and no
 * CSS: it cannot see a control pushed off the side of a phone, and axe cannot judge contrast
 * there at all. This spec is the run that can. The API is answered in the browser, so it needs
 * no Portal — `vite preview` serves the built bundle and nothing else.
 *
 * Sizes are the two a person actually meets and the repository already uses: a phone at 400 CSS
 * px, and a 1280 px window at 1.5× browser zoom, which is a viewport 853 CSS px wide. Nothing is
 * screenshotted: overflow, reachability and axe are judged, so there is no baseline to regenerate
 * in an environment with different fonts.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const COPY = "air-v2";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@hel.fi",
  roles: ["portal-editor"],
};

const WORKSPACE = {
  name: COPY,
  title: "Air cleanup before the winter season",
  project: PROJECT,
  owner: IDENTITY.email,
  branch: `workspace/${COPY}`,
  baseRevision: "base1",
  scope: { kind: "project" },
  previewState: "none",
  createdAt: "2026-09-18T09:00:00Z",
  // A week from the run: a fixed date expires the copy the day it passes (T-2729).
  expiresAt: new Date(Date.now() + 7 * 24 * 3600 * 1000).toISOString(),
  changes: 3,
};

/** Two conflicting fields on one file: the widest thing the chooser ever has to lay out. */
const COMPARISON = {
  files: [
    {
      path: `projects/${PROJECT}/pipelines/air-quality-ingest/pipeline.yaml`,
      kind: "Pipeline",
      operation: "Update",
      lane: "red",
      fields: [{ path: "spec.schedule", from: "every 10 seconds", to: "every 30 seconds" }],
    },
  ],
  conflicts: [
    {
      path: `projects/${PROJECT}/pipelines/air-quality-ingest/pipeline.yaml`,
      fields: [
        {
          path: "spec.compute.mapping.window",
          ours: "root.observedAt = this.timestamp.ts_parse(\"2006-01-02T15:04:05Z\")",
          theirs: "root.observedAt = this.timestamp.ts_strptime(\"%Y-%m-%dT%H:%M:%SZ\")",
          base: "root.observedAt = this.timestamp",
        },
        { path: "spec.schedule", ours: "every 30 seconds", theirs: "every 60 seconds", base: "every 10 seconds" },
      ],
    },
  ],
};

async function stubApi(page: Page): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "List", items: [{ name: PROJECT }] });
    if (path === `/api/v1/projects/${PROJECT}`) {
      return json({ apiVersion: API, kind: "Project", metadata: { name: PROJECT }, spec: {}, status: {} });
    }
    if (path.endsWith(`/workspaces/${COPY}/compare`)) return json(COMPARISON);
    if (path.endsWith(`/workspaces/${COPY}`)) return json(WORKSPACE);
    return json({ apiVersion: API, kind: "List", items: [] });
  });
}

/** How far the page can be scrolled sideways: anything above a rounding pixel is overflow. */
async function sidewaysOverflow(page: Page): Promise<number> {
  return page.evaluate(() => {
    const root = document.documentElement;
    return Math.max(0, root.scrollWidth - root.clientWidth);
  });
}

const SIZES = [
  { name: "a phone at 400px", viewport: { width: 400, height: 860 } },
  // 1280 px laid out at 1.5× browser zoom is a viewport 853 CSS px wide (UI-15).
  { name: "1280px at 1.5x zoom", viewport: { width: 853, height: 480 } },
];

test.describe("the copy bar and the conflict chooser", () => {
  for (const size of SIZES) {
    test.describe(size.name, () => {
      test.use({ viewport: size.viewport });

      test("the bar shows every control and pushes nothing off the side", async ({ page }) => {
        await stubApi(page);
        await page.goto(`/projects/${PROJECT}/spaces?workspace=${COPY}&lang=en`);

        const bar = page.getByRole("region", { name: "Copy" });
        await expect(bar).toBeVisible();
        // The title is long on purpose: the bar wraps rather than clipping or scrolling.
        await expect(bar.getByText("Air cleanup before the winter season")).toBeVisible();
        for (const control of ["Try it", "Compare", "Bring back", "Leave"]) {
          await expect(bar.getByRole(control === "Leave" ? "button" : "link", { name: control })).toBeVisible();
        }
        expect(await sidewaysOverflow(page)).toBe(0);
      });

      test("the conflict chooser answers both fields without a sideways scroll", async ({ page }) => {
        await stubApi(page);
        await page.goto(`/projects/${PROJECT}/workspaces/${COPY}/bring-back?lang=en`);

        const groups = page.getByRole("radiogroup");
        await expect(groups).toHaveCount(2);
        for (let index = 0; index < 2; index += 1) {
          const group = groups.nth(index);
          await expect(group.getByRole("radio")).toHaveCount(2);
          // Answering by click is what a mouse does; the keyboard path is in the unit tests.
          await group.getByRole("radio").first().check();
          await expect(group.getByRole("radio").first()).toBeChecked();
        }
        const update = page.getByRole("button", { name: "Update from the project" });
        await expect(update).toBeVisible();
        await expect(update).not.toHaveAttribute("aria-disabled", "true");
        expect(await sidewaysOverflow(page)).toBe(0);
      });
    });
  }

  test("the bar and the chooser have no axe violations", async ({ page }) => {
    await stubApi(page);

    await page.goto(`/projects/${PROJECT}/spaces?workspace=${COPY}&lang=en`);
    await expect(page.getByRole("region", { name: "Copy" })).toBeVisible();
    // The bar is the Shell's and draws first; the page under it loads with its section (T-3280),
    // so the audit waits for the page a person reads, not for the bar alone.
    await expect(page.locator("main h1").first()).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);

    await page.goto(`/projects/${PROJECT}/workspaces/${COPY}/bring-back?lang=en`);
    await expect(page.getByRole("radiogroup").first()).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
  });

  test("the new address opens the dialog over the list, and Escape goes back to it", async ({ page }) => {
    // T-2749: `/workspaces/new` crashed on 'reading title' as a kind's create form.
    await stubApi(page);
    await page.goto(`/projects/${PROJECT}/workspaces/new?lang=en`);
    const dialog = page.getByRole("dialog", { name: "Work on a copy" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByLabel(/^Name/)).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(page).toHaveURL(new RegExp(`/projects/${PROJECT}/workspaces(\\?|$)`));
  });

  test("the copy that is gone is announced, not only shown", async ({ page }) => {
    // A copy discarded by its owner while somebody else is reading a page of it: the line that
    // replaces the bar is a live region, so a screen reader says it without being asked
    // (T-1254). Its visibility is what this run can see; that it is `status` is the assertion.
    await page.route("**/api/v1/**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (body: unknown, status = 200) =>
        route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
      if (path.endsWith("/auth/me")) return json(IDENTITY);
      if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "List", items: [{ name: PROJECT }] });
      if (path.endsWith(`/workspaces/${COPY}`)) return json({ title: "Not Found", status: 404 }, 404);
      return json({ apiVersion: API, kind: "List", items: [] });
    });
    await page.goto(`/projects/${PROJECT}/spaces?workspace=${COPY}&lang=en`);

    const notice = page.getByRole("status").filter({ hasText: "expired or was discarded" });
    await expect(notice).toBeVisible();
    await expect(notice.getByRole("button", { name: "Leave" })).toBeVisible();
  });
});

/**
 * The copies list, a copy's comparison and its preview, each opened at its own address the way
 * a person arrives from a link (T-2729; UI-15, UI-44, PF-83). The stub keeps state, so what a
 * click sends is what the next read answers: a discarded copy leaves the list, a started
 * preview runs, a stopped one does not.
 */
test.describe("the copies, what a copy changes, and trying it", () => {
  const OTHER = { ...WORKSPACE, name: "bikes-fix", title: "Bike docks", owner: "matti@hel.fi" };

  interface World {
    workspaces: (typeof WORKSPACE)[];
    preview: Record<string, unknown>;
    sent: string[];
  }

  async function stubCopies(page: Page, world: World): Promise<void> {
    await page.route("**/api/v1/**", async (route) => {
      const request = route.request();
      const path = new URL(request.url()).pathname;
      const method = request.method();
      const json = (body: unknown, status = 200) =>
        route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
      const base = `/api/v1/projects/${PROJECT}/workspaces`;

      if (path.endsWith("/auth/me")) return json(IDENTITY);
      if (path === "/api/v1/projects") return json({ apiVersion: API, kind: "List", items: [{ name: PROJECT }] });
      if (method !== "GET") world.sent.push(`${method} ${path}`);
      if (path === base) return json({ items: world.workspaces });
      if (path.startsWith(`${base}/`) && path.endsWith("/preview")) {
        if (method === "POST") {
          world.preview = {
            state: "running",
            prefix: `ws-${COPY}-`,
            endpoints: [
              { name: "public-air", slug: "minted", url: "https://preview.example.org/api/endpoint/minted", originSlug: "origin" },
            ],
            pausedPipelines: ["air-quality-ingest"],
          };
          return json(world.preview, 202);
        }
        if (method === "DELETE") {
          world.preview = { state: "none", endpoints: [], pausedPipelines: [] };
          return route.fulfill({ status: 204 });
        }
        return json(world.preview);
      }
      if (path.endsWith(`/workspaces/${COPY}/compare`)) return json(COMPARISON);
      const one = world.workspaces.find((workspace) => path === `${base}/${workspace.name}`);
      if (one && method === "DELETE") {
        world.workspaces = world.workspaces.filter((workspace) => workspace !== one);
        return route.fulfill({ status: 204 });
      }
      if (one) return json(one);
      return json({ apiVersion: API, kind: "List", items: [] });
    });
  }

  const fresh = (): World => ({
    workspaces: [WORKSPACE, OTHER],
    preview: { state: "none", endpoints: [], pausedPipelines: [] },
    sent: [],
  });

  test("the list separates my copies from other people's, and Discard asks before it sends", async ({ page }) => {
    const world = fresh();
    await stubCopies(page, world);
    await page.goto(`/projects/${PROJECT}/workspaces?lang=en`);

    await expect(page.getByRole("heading", { level: 1, name: "Copies" })).toBeVisible();
    const mine = page.getByRole("heading", { level: 2, name: "My copies" }).locator("..");
    const others = page.getByRole("heading", { level: 2, name: "Other people's copies" }).locator("..");
    await expect(mine.getByRole("row", { name: /Air cleanup before the winter season/ })).toBeVisible();
    await expect(others.getByRole("row", { name: /Bike docks/ })).toBeVisible();
    // Only the owner throws a copy away (AG-11 keeps it from agents; the list keeps it from others).
    await expect(others.getByRole("button", { name: "Discard" })).toHaveCount(0);
    expect(await axeViolations(page)).toEqual([]);

    await mine.getByRole("button", { name: "Discard" }).click();
    const dialog = page.getByRole("alertdialog", { name: "Discard" });
    await expect(dialog).toContainText(`Discard the copy ${COPY}?`);
    expect(world.sent, "nothing is sent before the person confirms").toEqual([]);
    await dialog.getByRole("button", { name: "Discard" }).click();

    await expect(dialog).toBeHidden();
    await expect(page.getByRole("heading", { level: 2, name: "My copies" })).toHaveCount(0);
    await expect(others.getByRole("row", { name: /Bike docks/ })).toBeVisible();
    expect(world.sent).toEqual([`DELETE /api/v1/projects/${PROJECT}/workspaces/${COPY}`]);
  });

  test("Compare counts what the copy changes and sends a conflict to the bring back page", async ({ page }) => {
    await stubCopies(page, fresh());
    await page.goto(`/projects/${PROJECT}/workspaces/${COPY}/compare?lang=en`);

    await expect(page.getByRole("heading", { level: 1, name: `What the copy ${COPY} changes` })).toBeVisible();
    await expect(page.getByText("0 added, 1 changed, 0 removed")).toBeVisible();
    await expect(page.getByText(/1 file also changed in the project/)).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);

    await page.getByRole("link", { name: "Resolve on the bring back page" }).click();
    await expect(page).toHaveURL(new RegExp(`/workspaces/${COPY}/bring-back`));
    await expect(page.getByRole("radiogroup").first()).toBeVisible();
  });

  test("Try it starts the copy's preview, says where it answers, and stops it", async ({ page }) => {
    const world = fresh();
    await stubCopies(page, world);
    await page.goto(`/projects/${PROJECT}/workspaces/${COPY}/try-it?lang=en`);

    await expect(page.getByRole("heading", { level: 1, name: "Try the copy" })).toBeVisible();
    await expect(page.getByTestId("preview-state")).toHaveText("Not started");
    await page.getByRole("button", { name: "Start the preview" }).click();

    await expect(page.getByTestId("preview-state")).toHaveText("Running");
    await expect(page.getByRole("link", { name: /preview\.example\.org/ })).toBeVisible();
    await expect(page.getByText("air-quality-ingest")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);

    await page.getByRole("button", { name: "Stop" }).click();
    await expect(page.getByTestId("preview-state")).toHaveText("Not started");
    await expect(page.getByRole("button", { name: "Start the preview" })).toBeVisible();
    expect(world.sent).toEqual([
      `POST /api/v1/projects/${PROJECT}/workspaces/${COPY}/preview`,
      `DELETE /api/v1/projects/${PROJECT}/workspaces/${COPY}/preview`,
    ]);
  });

  test("someone else's copy offers no preview to start and says whose it is to run", async ({ page }) => {
    const world = fresh();
    await stubCopies(page, world);
    await page.goto(`/projects/${PROJECT}/workspaces/${OTHER.name}/try-it?lang=en`);

    await expect(page.getByText("Only the person who started this copy runs its preview.")).toBeVisible();
    await expect(page.getByRole("button", { name: "Start the preview" })).toHaveCount(0);
    expect(world.sent).toEqual([]);
  });
});
