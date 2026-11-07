import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// PL-115, EP-55, T-5615: a person refused by an Endpoint asks its space's owners for a read on
// the Endpoint's page; an owner approves it there; the person reads the answer. `vite preview`
// has no Portal behind it: the API is answered in the browser by one stateful stub, everything
// above it is the real page.

const SHOTS = process.env.JC_SHOTS_DIR;
const PROJECT = "banskabystrica";
const NAME = "banskabystrica-mesto";
const PAGE = `/projects/${PROJECT}/endpoints/${NAME}?lang=en`;

const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: NAME, namespace: PROJECT },
  spec: {
    contextSpaceRef: "mesto",
    slug: "mesto-k7m2qz4tv6xh3n5jb2ryabcd",
    audience: "project",
  },
};
const TYPES = ["AirQualityObserved", "ParkingSpot", "PointOfInterest"];

interface World {
  who: string;
  items: Record<string, unknown>[];
  refuse?: { status: number; body: Record<string, unknown> };
  refusal?: string;
  types?: string[];
}

function request(state: string, extra: Record<string, unknown> = {}) {
  return {
    id: "ar-0a1b2c3d4e5f",
    pipeline: null,
    person: "demo.viewer",
    account: "demo.viewer",
    endpoint: { project: PROJECT, name: NAME },
    space: "mesto",
    operations: "read",
    types: [{ type: "ParkingSpot", attrs: [] }],
    until: "2027-01-31T23:59:59Z",
    reason: "Weekly parking report for the district office",
    state,
    requestedBy: "demo.viewer",
    requestedAt: "2026-10-25T08:00:00Z",
    decidedBy: null,
    decidedAt: null,
    decisionReason: null,
    change: null,
    ...extra,
  };
}

async function stubApi(page: Page, world: World): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const req = route.request();
    const path = new URL(req.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    if (path.endsWith("/auth/me"))
      return json({
        subject: world.who,
        username: world.who,
        name: world.who,
        email: `${world.who}@bb.sk`,
        roles: [],
      });
    if (path.endsWith("/permissions/me"))
      return json({ project: PROJECT, bootstrap: true, grants: [] });
    if (
      path === `/api/v1/projects/${PROJECT}/endpoints/${NAME}/access-requests`
    ) {
      if (req.method() === "POST") {
        if (world.refuse)
          return route.fulfill({
            status: world.refuse.status,
            contentType: "application/problem+json",
            body: JSON.stringify(world.refuse.body),
          });
        const body = req.postDataJSON() as { types: string[]; reason: string };
        const filed = request("pending", {
          types: body.types.map((type) => ({ type, attrs: [] })),
          reason: body.reason,
        });
        world.items = [filed];
        return json(filed, 201);
      }
      return json({
        types: world.types ?? TYPES,
        refusal: world.refusal ?? null,
        items: world.who === "demo.viewer" ? world.items : [],
      });
    }
    if (path === `/api/v1/projects/${PROJECT}/access-requests`)
      return json({ items: world.items });
    if (path.endsWith("/approve") && req.method() === "POST") {
      world.items = world.items.map((item) => ({
        ...item,
        state: "approved",
        decidedBy: world.who,
        change: "chg-00000042",
      }));
      return json(
        {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Change",
          metadata: { name: "chg-00000042", namespace: PROJECT },
          status: {
            lane: "red",
            phase: "PendingApproval",
            plan: { create: 1, update: 0, delete: 0 },
          },
        },
        202,
      );
    }
    if (path.endsWith(`/endpoints/${NAME}`)) return json(ENDPOINT);
    return json({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "List",
      items: [],
    });
  });
}

async function shot(page: Page, name: string, width: number): Promise<void> {
  if (SHOTS)
    await page.screenshot({
      path: `${SHOTS}/endpoint-access-${name}-${width}.png`,
      fullPage: false,
    });
}

async function fill(page: Page, type: string, reason: string): Promise<void> {
  const section = page.getByTestId("endpoint-access-request");
  const types = section.getByRole("combobox", { name: "Entity types" });
  await types.click();
  await page
    .getByRole("option", { name: new RegExp(type) })
    .first()
    .click();
  await types.press("Escape");
  await section.getByLabel(/^Reason/).fill(reason);
}

for (const [width, height] of [
  [1440, 900],
  [400, 860],
] as const) {
  test.describe(`a person's access request at ${width}`, () => {
    test.use({ viewport: { width, height } });

    test("a viewer asks, a steward approves, the viewer reads the answer", async ({
      page,
    }) => {
      const world: World = { who: "demo.viewer", items: [] };
      await stubApi(page, world);
      await page.goto(PAGE);
      const section = page.getByTestId("endpoint-access-request");
      await expect(section.getByText("Ask for a read")).toBeVisible();
      await section.scrollIntoViewIfNeeded();
      await shot(page, "empty", width);
      expect(await axeViolations(page)).toEqual([]);

      await section.getByRole("button", { name: "Send request" }).click();
      await expect(
        section.getByText("Choose at least one type."),
      ).toBeVisible();
      await shot(page, "missing", width);

      await fill(
        page,
        "ParkingSpot",
        "Weekly parking report for the district office",
      );
      await section.getByLabel(/^Until/).fill("2027-01-31");
      await shot(page, "filled", width);
      await section.getByRole("button", { name: "Send request" }).click();
      await expect(
        section.getByText("Request sent.", { exact: false }),
      ).toBeVisible();
      await expect(section.getByText("Waiting for the owner")).toBeVisible();
      await shot(page, "pending", width);

      world.who = "demo.steward";
      await page.reload();
      const approve = page.getByRole("button", {
        name: "Approve access for demo.viewer",
      });
      await approve.scrollIntoViewIfNeeded();
      await shot(page, "inbox", width);
      await approve.click();
      await expect(page.getByText(/chg-00000042/).first()).toBeVisible();
      await shot(page, "approved-change", width);

      world.who = "demo.viewer";
      await page.reload();
      await expect(
        section.getByText("Approved", { exact: true }),
      ).toBeVisible();
      await expect(
        section.getByText(/the grant applies once its change is merged/),
      ).toBeVisible();
      await section.scrollIntoViewIfNeeded();
      await shot(page, "approved", width);
    });

    test("the server's refusal of a field shows beside it", async ({
      page,
    }) => {
      await stubApi(page, {
        who: "demo.viewer",
        items: [],
        refuse: {
          status: 422,
          body: {
            title: "Unprocessable",
            status: 422,
            detail: "the request is not complete (EP-55)",
            errors: ["until: until has passed; choose a later end or none"],
          },
        },
      });
      await page.goto(PAGE);
      const section = page.getByTestId("endpoint-access-request");
      await fill(page, "AirQualityObserved", "Air report");
      await section.getByLabel(/^Until/).fill("2020-01-31");
      await section.getByRole("button", { name: "Send request" }).click();
      await expect(
        section.getByText("until has passed; choose a later end or none"),
      ).toBeVisible();
      await shot(page, "error", width);
    });

    test("an Endpoint no grant can reach says why instead of a form", async ({
      page,
    }) => {
      await stubApi(page, {
        who: "demo.viewer",
        items: [],
        refusal:
          "this endpoint serves only the policies it names, so a new grant would not reach it; ask its owners to add one to its policy list",
      });
      await page.goto(PAGE);
      const section = page.getByTestId("endpoint-access-request");
      await expect(
        section.getByText(/serves only the policies it names/),
      ).toBeVisible();
      await expect(
        section.getByRole("button", { name: "Send request" }),
      ).toHaveCount(0);
      await section.scrollIntoViewIfNeeded();
      await shot(page, "refusal", width);
    });

    test("a space with no model says what to do next", async ({ page }) => {
      await stubApi(page, { who: "demo.viewer", items: [], types: [] });
      await page.goto(PAGE);
      const section = page.getByTestId("endpoint-access-request");
      await expect(
        section.getByText(/has no compiled data model/),
      ).toBeVisible();
      await section.scrollIntoViewIfNeeded();
      await shot(page, "no-types", width);
    });
  });
}
