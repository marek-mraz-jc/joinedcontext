/**
 * T-5537 — an analyst consumes a department dataset, on dev, as a person (not a ServiceAccount).
 *
 * The analyst is `demo.viewer`, who holds no grant on the target Endpoint (by default
 * `banskabystrica-mesto`, audience `organization`, whose `/access` lists no permission for them).
 * The walk, lapped:
 *
 * 1. find the dataset in the catalogue and in the project's Endpoint list;
 * 2. open it and meet the refusal: what it says, and whether the page offers a way to ask;
 * 3. the steward grants the analyst read on one type through a Policy, a second person approves,
 *    and the analyst's `file.csv` is polled until it answers 200 (the time from the proposal to
 *    the first 200);
 * 4. the analyst downloads the CSV from the Endpoint page (bytes, seconds).
 *
 * The script half (NGSI-LD paging, temporal, the 429) is
 * `joinedcontext-conformance/tests/e2e/test_walk_analyst.py`, run between the grant and the revoke.
 * afterAll removes the Policy and checks the analyst is refused again.
 *
 * Steps 1-2 need VIEWER_PASSWORD only. Steps 3-4 sign in as demo.steward and demo.approver, whose
 * sign-in asks for a TOTP (deployment T-5120): PORTAL_TOTP and APPROVER_TOTP come from the
 * cluster Secrets at run time, so the cluster holder runs them (T-5109).
 *
 *   SHOTS_DIR=... VIEWER_PASSWORD=... PORTAL_PASSWORD=... PORTAL_TOTP=... APPROVER_PASSWORD=... \
 *     APPROVER_TOTP=... npx playwright test --config playwright.live.config.ts e2e/live/walk-analyst.spec.ts
 */
import { appendFileSync, mkdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import type { Browser, BrowserContext, Page } from "@playwright/test";
import {
  APPROVER,
  STEWARD,
  VIEWER,
  approve,
  checkManifest,
  csrf,
  removeCompletely,
  signIn,
} from "./portal";
import { shot } from "./shoot";

test.setTimeout(900_000);
test.describe.configure({ mode: "serial" });

const PROJECT = process.env.WALK_PROJECT ?? "banskabystrica";
const ENDPOINT = process.env.WALK_ENDPOINT ?? "banskabystrica-mesto";
const TYPE = process.env.WALK_TYPE ?? "KeyPerformanceIndicator";
const POLICY = `walk-analyst-${Date.now().toString(36)}`;
const WIDTHS = [1440, 400];

let viewer: { context: BrowserContext; page: Page } | undefined;
let steward: { context: BrowserContext; page: Page } | undefined;
let slug = "";
let granted = false;

/** One row of the step table: printed, and kept in `$SHOTS_DIR/laps.jsonl` beside the shots. */
function lap(
  step: string,
  started: number,
  more: Record<string, unknown> = {},
): void {
  const row = {
    step,
    seconds: Math.round(Date.now() - started) / 1000,
    ...more,
  };
  console.log(`LAP ${JSON.stringify(row)}`);
  const dir = process.env.SHOTS_DIR;
  if (dir) {
    mkdirSync(dir, { recursive: true });
    appendFileSync(join(dir, "laps.jsonl"), `${JSON.stringify(row)}\n`);
  }
}

async function shots(page: Page, route: string, state: string): Promise<void> {
  for (const width of WIDTHS) {
    await page.setViewportSize({ width, height: 900 });
    await shot(page, {
      area: "walk-analyst",
      route,
      state,
      who: "viewer",
      lang: "en",
    });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

async function csvStatus(page: Page): Promise<number> {
  return (
    await page.request.get(
      `/api/endpoint/${slug}/file.csv?type=${encodeURIComponent(TYPE)}&limit=1`,
    )
  ).status();
}

async function signedIn(
  browser: Browser,
  who: typeof STEWARD,
  path: string,
): Promise<{ context: BrowserContext; page: Page }> {
  if (!who.totp) {
    throw new Error(
      `${who.user} signs in with a TOTP: set its seed from the cluster Secret (T-5120); the cluster holder runs this step`,
    );
  }
  return signIn(browser, who, path);
}

test.afterAll(async ({ browser }) => {
  if (granted) {
    steward ??= await signedIn(
      browser,
      STEWARD,
      `/projects/${PROJECT}/policies?lang=en`,
    );
    await removeCompletely(steward, PROJECT, "policies", POLICY);
    if (viewer) {
      await expect
        .poll(() => csvStatus(viewer!.page), {
          message: "the analyst still reads after the revoke",
          timeout: 300_000,
          intervals: [5_000],
        })
        .toBe(403);
    }
  }
  await steward?.context.close();
  await viewer?.context.close();
});

test("the analyst finds the dataset in the catalogue and in the Endpoint list", async ({
  browser,
}) => {
  let started = Date.now();
  viewer = await signIn(
    browser,
    VIEWER,
    `/projects/${PROJECT}/endpoints?lang=en`,
  );
  const { page } = viewer;
  const answer = await page.request.get(
    `/api/v1/projects/${PROJECT}/endpoints/${ENDPOINT}`,
  );
  expect(answer.status(), "the viewer may read the Endpoint's manifest").toBe(
    200,
  );
  const manifest = (await answer.json()) as {
    spec: { slug: string; contextSpaceRef: unknown };
    metadata: { title?: Record<string, string> };
  };
  slug = manifest.spec.slug;
  const title = manifest.metadata.title?.en ?? ENDPOINT;
  await expect(
    page
      .getByRole("link", {
        name: new RegExp(
          title.slice(0, 20).replace(/[.*+?^${}()|[\]\\]/g, "\\$&"),
          "i",
        ),
      })
      .first(),
  ).toBeVisible({ timeout: 60_000 });
  await shots(page, `projects/${PROJECT}/endpoints`, "listed");
  lap("1a Endpoint list", started, { title });

  started = Date.now();
  await page.goto("/catalogue?lang=en", { waitUntil: "load" });
  await page.getByRole("searchbox").first().fill(title.split(/\s+/)[0]);
  await page.waitForTimeout(1_500);
  // An `organization` Endpoint is no open dataset: the catalogue lists only what is published,
  // so whether it shows here is what the walk records, not what it requires.
  const inCatalogue = await page.getByText(title, { exact: false }).count();
  await shots(page, "catalogue", inCatalogue ? "found" : "not-listed");
  lap("1b catalogue search", started, { listed: inCatalogue > 0 });
});

test("without a grant every read is refused and the page says why", async () => {
  const started = Date.now();
  const { page } = viewer!;
  expect(await csvStatus(page), "file.csv before the grant").toBe(403);
  const entities = await page.request.get(
    `/api/endpoint/${slug}/ngsi-ld/v1/entities?type=${encodeURIComponent(TYPE)}&limit=1`,
  );
  expect(entities.status(), "NGSI-LD before the grant").toBe(403);
  const hint = ((await entities.json()) as { hint?: string }).hint ?? "";
  await page.goto(`/projects/${PROJECT}/endpoints/${ENDPOINT}?lang=en`, {
    waitUntil: "load",
  });
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible({
    timeout: 60_000,
  });
  const askControl = await page
    .getByRole("button", { name: /request access|ask for access/i })
    .count();
  await shots(page, `projects/${PROJECT}/endpoints/${ENDPOINT}`, "refused");
  lap("2 refusal", started, { hint, requestAccessControl: askControl > 0 });
});

test("the steward grants read, a second person approves, and the analyst's CSV answers", async ({
  browser,
}) => {
  const started = Date.now();
  steward = await signedIn(
    browser,
    STEWARD,
    `/projects/${PROJECT}/policies?lang=en`,
  );
  const endpoint = (await (
    await steward.page.request.get(
      `/api/v1/projects/${PROJECT}/endpoints/${ENDPOINT}`,
    )
  ).json()) as {
    spec: { contextSpaceRef: unknown };
  };
  const policy = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Policy",
    metadata: { name: POLICY, namespace: PROJECT },
    spec: {
      contextSpaceRef: endpoint.spec.contextSpaceRef,
      assigner: "did:web:{orgDomain}",
      assignee: { kind: "user", id: VIEWER.user },
      operations: ["retrieveOps"],
      information: [{ entities: [{ type: TYPE }] }],
      // A day at most: should afterAll never run, the grant ends by itself.
      validity: {
        to: new Date(Date.now() + 86_400_000)
          .toISOString()
          .replace(/\.\d+Z$/, "Z"),
      },
    },
  };
  await checkManifest(steward.page, steward.context, PROJECT, policy);
  const answer = await steward.page.request.post(
    `/api/v1/projects/${PROJECT}/policies`,
    {
      headers: { "x-csrf-token": await csrf(steward.context) },
      data: policy,
    },
  );
  expect(answer.status(), await answer.text()).toBe(202);
  const change =
    ((await answer.json()) as { metadata?: { name?: string } }).metadata
      ?.name ?? "";
  expect(change, "the grant's proposal names its change").not.toBe("");
  granted = true;
  lap("3a grant proposed", started, { change });

  const approver = await signedIn(
    browser,
    APPROVER,
    `/projects/${PROJECT}/approvals?lang=en`,
  );
  try {
    await approve(approver.page, PROJECT, change, POLICY);
  } finally {
    await approver.context.close();
  }
  lap("3b grant approved", started);
  await expect
    .poll(() => csvStatus(viewer!.page), {
      message: "the analyst's file.csv after the approval",
      timeout: 600_000,
      intervals: [5_000],
    })
    .toBe(200);
  lap("3c proposal to first 200", started);
});

test("the analyst downloads the CSV from the Endpoint page", async () => {
  const started = Date.now();
  const { page } = viewer!;
  await page.goto(`/projects/${PROJECT}/endpoints/${ENDPOINT}?lang=en`, {
    waitUntil: "load",
  });
  const link = page.getByRole("link", { name: "Download these rows as CSV" });
  await expect(link).toBeVisible({ timeout: 90_000 });
  await shots(page, `projects/${PROJECT}/endpoints/${ENDPOINT}`, "granted");
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    link.click(),
  ]);
  const file = await download.path();
  const { size } = statSync(file);
  expect(size, "the CSV holds more than its header").toBeGreaterThan(100);
  lap("4 CSV from the UI", started, { bytes: size });
});
