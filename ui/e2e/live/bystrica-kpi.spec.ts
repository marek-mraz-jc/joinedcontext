/**
 * The Banská Bystrica demonstration, walked end to end on dev (T-2309, OPS-46, TS-19).
 *
 * Two public bodies, each ingesting its own published open data into its own Context Space, each
 * computing its own indicators into its own KPI space with its own pipeline, and one application
 * in `bbsk` showing both. The journey opens each space, then the application, and then does the
 * one check a viewer cannot do for themselves: it reads the indicators straight out of each KPI
 * space and asserts the cards show those numbers and no others.
 *
 * Two failures this exists to catch, both invisible on screen:
 *
 * - a card whose number is not the number in the space it names — a dashboard drifting from its
 *   source looks exactly like a dashboard;
 * - a card of one body carrying the other's value. The region has roughly 607 581 inhabitants and
 *   the city 72 123; either number under the other's heading reads as a fact.
 */
import { expect, request, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, goSignedIn, signIn } from "./portal";

const APP = "bbsk-ukazovatele";
/**
 * Where applications are served: the apex, not the Portal host, since T-2476 (the Portal answers
 * `/apps/*` with a 308 there). `JC_PORTAL_APPS_URL` on the cluster; `APPS_URL` here.
 */
const APPS_URL = process.env.APPS_URL ?? "https://dev.joinedcontext.com";

/** The domain of the Organization the two projects sit on today; see the id check below (T-2455). */
const ORG_DOMAIN = "hel.fi";

/** What each body publishes, and where. `Development/10` §1, §2 and §4 are the contract. */
const BODIES = [
  {
    body: "bbsk",
    project: "bbsk",
    raw: { space: "bbsk-kraj", type: "KeyPerformanceIndicator" },
    kpi: { space: "bbsk-kpi", count: 28 },
    domain: "bbsk.sk",
  },
  {
    body: "banskabystrica",
    project: "banskabystrica",
    raw: { space: "banskabystrica-mesto", type: "KeyPerformanceIndicator" },
    kpi: { space: "banskabystrica-kpi", count: 3 },
    domain: "banskabystrica.sk",
  },
] as const;

/**
 * The digits of a number as any locale would print it with at most two decimals.
 *
 * The application formats with `Intl` in whichever language the served configuration asks for, so
 * the separators differ — `607 581` or `607,581`, `35,5` or `35.5`. Comparing digit sequences
 * compares the number and nothing else, and still fails on a different number, a rounded-away
 * decimal or a transposition.
 */
function digits(value: number): string {
  return String(Math.round(value * 100) / 100).replace(/\D/g, "");
}

/** One row of `GET /api/v1/endpoints`: only the two fields this journey needs. */
interface EndpointItem {
  spec?: { contextSpaceRef?: string; slug?: string };
}

/** A `KeyPerformanceIndicator` as the gateway serialises it: only the parts this journey reads. */
interface NgsiEntity {
  id: string;
  currentValue?: { value: number | string; unitCode?: string };
}

/**
 * The slug of the Endpoint that publishes one space, as the person's own session may see it.
 *
 * `GET /api/v1/endpoints` answers what this caller may read and nothing else, so a space whose
 * endpoint is missing from the answer is one the demonstration cannot show at all — which is what
 * the assertion says.
 */
async function endpointOf(page: Page, space: string): Promise<string> {
  const response = await page.request.get("/api/v1/endpoints");
  expect(response.ok(), `/api/v1/endpoints answered ${response.status()}`).toBe(true);
  const items = ((await response.json()) as { items?: EndpointItem[] }).items ?? [];
  const found = items.find((item) => item.spec?.contextSpaceRef === space);
  expect(found, `no Endpoint this person may read publishes ${space}`).toBeDefined();
  const slug = found!.spec?.slug;
  expect(slug, `the Endpoint of ${space} has no slug`).toBeTruthy();
  return slug!;
}

/**
 * Every indicator one KPI space holds, read through the endpoint the Portal itself reads it
 * through (`/api/endpoint/{slug}`, `components/endpoints/links.tsx`) and with the person's own
 * session.
 *
 * Not `/cs/{space}`: that surface answers each caller what their audience allows, and the city's
 * indicators are `audience: project-list` for the region alone (`banskabystrica/endpoint-kpi.yaml`,
 * EP-15) — a person reading them there gets the same `404` as for a space that does not exist,
 * which is the platform being right (SP-06). The region's are `public` and answer either way.
 */
async function indicatorsInSpace(
  page: Page,
  space: string,
): Promise<Map<string, { value: number | string; unitCode?: string }>> {
  const slug = await endpointOf(page, space);
  const url = `/api/endpoint/${encodeURIComponent(slug)}/ngsi-ld/v1/entities?type=KeyPerformanceIndicator&limit=200`;
  const response = await page.request.get(url, { headers: { Accept: "application/ld+json" } });
  expect(
    response.ok(),
    `${space} answered ${response.status()} at its own endpoint; the demonstration needs it readable by the demo person`,
  ).toBe(true);
  const entities = (await response.json()) as NgsiEntity[];
  expect(Array.isArray(entities), `${url} did not answer a list of entities`).toBe(true);
  return new Map(
    entities.map((entity) => [
      String(entity.id),
      { value: entity.currentValue?.value, unitCode: entity.currentValue?.unitCode },
    ]),
  );
}

test("both bodies ingest, compute and publish their own indicators, each into its own space", async ({
  browser,
}) => {
  test.setTimeout(600_000);
  const { context, page } = await signIn(browser, STEWARD, "/projects/bbsk/spaces?lang=en");

  const inSpace = new Map<string, Map<string, { value: number | string; unitCode?: string }>>();

  for (const body of BODIES) {
    // The project selector says which body's spaces are on screen; a figure read under the wrong
    // one is the mistake the whole demonstration is arranged against.
    await page.goto(`/projects/${body.project}/spaces?lang=en`, { waitUntil: "load" });
    await expect(page.getByRole("navigation", { name: "Main navigation" })).toBeVisible();
    await expect(page.getByText(body.project, { exact: true }).first()).toBeVisible();

    // The raw space: what the publisher published, ingested by the pipelines of T-2305.
    await page.goto(`/projects/${body.project}/spaces/${body.raw.space}?lang=en`, {
      waitUntil: "load",
    });
    // Inside the "Entity types" table, not anywhere on the page: the space page also lists the
    // policies of the space, and a policy that says what a steward may write to a
    // `KeyPerformanceIndicator` is a row whose text holds the type name too (T-2453).
    const types = page.getByRole("table", { name: "Entity types" });
    const rawRow = types.getByRole("row").filter({ hasText: body.raw.type });
    await expect(
      rawRow,
      `${body.raw.space} shows no ${body.raw.type}: nothing was ingested`,
    ).toBeVisible({ timeout: 60_000 });
    // The row is drawn before its count is known — each row asks the gateway for its own count
    // and says "Loading…" until the answer arrives — so the digits are read once there are
    // digits, and not at the moment the row appears (T-2453).
    await expect(rawRow, `${body.raw.space} never answered a count`).toContainText(/\d/, {
      timeout: 60_000,
    });
    expect(
      Number((await rawRow.innerText()).replace(/\D/g, "")),
      `${body.raw.space} holds no ${body.raw.type} entity`,
    ).toBeGreaterThan(0);

    // The KPI space: one entity per indicator per territory, and nothing else.
    await page.goto(`/projects/${body.project}/spaces/${body.kpi.space}?lang=en`, {
      waitUntil: "load",
    });
    const kpiRow = page
      .getByRole("table", { name: "Entity types" })
      .getByRole("row")
      .filter({ hasText: "KeyPerformanceIndicator" });
    await expect(kpiRow, `${body.kpi.space} shows no indicator`).toBeVisible({ timeout: 60_000 });

    const held = await indicatorsInSpace(page, body.kpi.space);
    expect(held.size, `${body.kpi.space} holds ${held.size} indicators, not ${body.kpi.count}`).toBe(
      body.kpi.count,
    );
    for (const id of held.keys()) {
      // The territory is the id's last segment and the space is the fifth: both are in the id
      // because the published schema has no attribute for either (PF-54). The space is what tells
      // the two bodies apart — the fourth segment is the *organization's* domain, and on dev both
      // projects sit on one Organization, so both read `hel.fi` while the contract
      // (`Development/10` §1) gives each body its own. Which of the two is right is the owner's,
      // and it is T-2455; the day it is answered, the accepted set below becomes `body.domain`
      // alone.
      const segments = id.split(":");
      expect(segments, id).toHaveLength(6);
      expect(segments[4], `${id} is in ${body.kpi.space} but names another space`).toBe(
        body.kpi.space,
      );
      expect([body.domain, ORG_DOMAIN], `${id} names an organization that publishes neither body`)
        .toContain(segments[3]);
    }
    inSpace.set(body.body, held);
  }

  // And the region's indicators are open data: `bbsk-kpi` is `audience: public`
  // (`bbsk/bbsk-endpoint-kpi.yaml`), so a person with no account at all reads them on the space
  // surface. This is the one claim of the demonstration that no signed-in check can make, so it
  // is made with a context that carries no session.
  const anonymous = await request.newContext({
    baseURL: process.env.PORTAL_URL ?? "https://portal.dev.joinedcontext.com",
  });
  try {
    const open = await anonymous.get(
      "/cs/bbsk-kpi/ngsi-ld/v1/entities?type=KeyPerformanceIndicator&limit=200",
      { headers: { Accept: "application/ld+json" } },
    );
    expect(open.status(), "the region's indicators are not open data to a caller with no account").toBe(200);
    expect((await open.json()) as NgsiEntity[], "the open read answers an empty region").toHaveLength(
      inSpace.get("bbsk")!.size,
    );
  } finally {
    await anonymous.dispose();
  }

  await context.close();
});

/**
 * The application, in the region's project, reading both bodies — `DEMO.md` step 6.
 *
 * The Portal image ships the bundle and its `integrity.json` under `JC_PORTAL_APPS_DIR`, the
 * configuration repository holds the App, and the host writes the `#jc-config` naming the region's
 * endpoint and the city's through `mesto-kpi` into the index it serves (T-2457).
 */
test("one application shows both bodies, each number the number in its own space", async ({
  browser,
}) => {
  test.setTimeout(600_000);
  // The spaces are read on the Portal, where the steward's session reads them; the application
  // is then opened on its own origin, and the sign-in walk waits for the application's heading —
  // an app page has no Portal navigation to wait for (T-2309).
  const { context, page } = await signIn(browser, STEWARD, "/projects/bbsk/spaces?lang=en");

  const inSpace = new Map<string, Map<string, { value: number | string; unitCode?: string }>>();
  for (const body of BODIES) {
    inSpace.set(body.body, await indicatorsInSpace(page, body.kpi.space));
  }

  await goSignedIn(page, STEWARD, `${APPS_URL}/apps/${APP}/`, (app) =>
    app.getByRole("heading", { level: 1 }),
  );

  for (const body of BODIES) {
    // The section and the cards are found by the ids the application mints, not by their words,
    // so the journey does not depend on which language the served configuration asks for.
    const section = page.locator(`section[aria-labelledby="body-${body.body}"]`);
    await expect(section, `the application shows no section for ${body.body}`).toBeVisible({
      timeout: 60_000,
    });

    const cards = section.locator("article[aria-labelledby^='card-urn:']");
    const held = inSpace.get(body.body)!;
    await expect(cards).toHaveCount(held.size, { timeout: 60_000 });

    const count = await cards.count();
    for (let index = 0; index < count; index += 1) {
      const card = cards.nth(index);
      const id = (await card.getAttribute("aria-labelledby"))!.replace(/^card-/, "");

      // A card of one body carrying the other's indicator: `held` was read from this body's own
      // space a moment ago, so membership is the whole check, and by the exact id rather than by
      // a prefix of it.
      const entity = held.get(id);
      expect(entity, `the card shows ${id}, which is not in ${body.kpi.space}`).toBeDefined();

      const reading = (await card.locator(".reading").innerText()).trim();
      if (typeof entity!.value === "number") {
        // The number on the card is the number in the space.
        expect(
          reading.replace(/\D/g, ""),
          `${id}: the card reads "${reading}" and the space holds ${entity!.value}`,
        ).toBe(digits(entity!.value));
        // And it is never a number without its unit.
        await expect(card.locator(".unit"), `${id} shows no unit`).not.toBeEmpty();
      } else {
        // "not measured": the words, and no zero standing in for a missing reading.
        expect(reading, `${id} holds no number but the card reads "${reading}"`).not.toMatch(/\d/);
        expect(entity!.unitCode, `${id} has no value but carries a unit`).toBeUndefined();
      }
    }
  }

  await context.close();
});
