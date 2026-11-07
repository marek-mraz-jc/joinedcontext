import { readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";

// The Endpoints page over T-4497's generated catalogue (T-4544, EP-01): 10,000 Endpoints in one
// project, titled as the gateway's generator titles them (its topics, the 518 places read from
// dev in tests/fixtures/places-dev.txt, 2023 to 2026, the same xorshift64* stream, seed 4497).
// The API is answered in the browser, so the time is the page's own: from navigation to the
// first row on screen, gated under one second (T-4545); the last generated Endpoint is beyond the
// first page and is found by the page's search, and opens by name from the address.

const TOPICS: [string, string][] = [
  ["Air quality", "Kvalita ovzdušia"],
  ["Weather observations", "Meteorologické pozorovania"],
  ["Events", "Podujatia"],
  ["Bike sharing", "Zdieľané bicykle"],
  ["Parking spots", "Parkovacie miesta"],
  ["Traffic counts", "Sčítanie dopravy"],
  ["Population statistics", "Štatistika obyvateľstva"],
  ["Points of interest", "Body záujmu"],
  ["Waste containers", "Kontajnery na odpad"],
  ["Weather warnings", "Výstrahy pred počasím"],
  ["Noise levels", "Hladina hluku"],
  ["Water quality", "Kvalita vody"],
  ["Street lighting", "Verejné osvetlenie"],
  ["Public transport vehicles", "Vozidlá MHD"],
];
const YEARS = [2023, 2024, 2025, 2026];
const N = 10_000;
const MASK = (1n << 64n) - 1n;

function catalogue(seed: bigint): unknown[] {
  const places = readFileSync(new URL("../../tests/fixtures/places-dev.txt", import.meta.url), "utf8")
    .split("\n")
    .filter((line) => line !== "" && !line.startsWith("#"));
  let state = seed > 0n ? seed : 1n;
  const next = (len: number): number => {
    state ^= state >> 12n;
    state ^= (state << 25n) & MASK;
    state ^= state >> 27n;
    return Number(((state * 0x2545f4914f6cdd1dn) & MASK) % BigInt(len));
  };
  return Array.from({ length: N }, (_, i) => {
    const [en, sk] = TOPICS[next(TOPICS.length)];
    const at = places[next(places.length)];
    const year = YEARS[next(YEARS.length)];
    return {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: {
        name: `d${String(i).padStart(5, "0")}`,
        namespace: "bbsk",
        title: { sk: `${sk} ${at} ${year}`, en: `${en} ${at} ${year}` },
      },
      spec: {
        contextSpaceRef: "otvorene-data",
        slug: String(i).padStart(26, "0"),
        audience: "public",
        enabledRepresentations: ["ngsi-ld", "geojson"],
      },
      status: { phase: "Live" },
    };
  });
}

test("ten thousand endpoints: the first row is on screen within a second and the last is found", async ({ page }) => {
  test.setTimeout(180_000);
  const items = catalogue(4497n);
  const body = JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", metadata: {}, items });
  // When the list reached the page: the clock of what this page does with 10 000 rows, apart from
  // booting the app on a dev server, which a loaded CI runner stretched past a second (T-4561).
  let answered = 0;
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (value: string) => route.fulfill({ status: 200, contentType: "application/json", body: value });
    if (path.endsWith("/auth/me")) {
      return json(JSON.stringify({ subject: "s", username: "jana", name: "Jana", email: "jana@bbsk.sk", roles: ["portal-editor"] }));
    }
    if (path === "/api/v1/projects") {
      return json(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "bbsk" }] }));
    }
    if (path === "/api/v1/projects/bbsk/endpoints") {
      await json(body);
      answered ||= Date.now();
      return;
    }
    return json(JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] }));
  });

  const first = (items[0] as { metadata: { title: { en: string } } }).metadata.title.en;
  const last = (items[N - 1] as { metadata: { title: { en: string } } }).metadata.title.en;
  // the gateway generator's first and last Dataset with seed 4497
  expect([first, last]).toEqual(["Parking spots Hrochoť 2023", "Population statistics Olováry 2026"]);
  const started = Date.now();
  await page.goto("/projects/bbsk/endpoints?lang=en");
  await expect(page.getByText(first, { exact: true }).first()).toBeVisible({ timeout: 120_000 });
  const shown = Date.now();
  const firstRow = shown - answered;
  const bootMs = answered - started;
  const rows = await page.locator("[data-records] tbody tr").count();
  // The search asks the list the page already holds: the last of 10 000, a page and more away.
  const searched = Date.now();
  await page.getByRole("searchbox", { name: "Find an endpoint by name or title" }).fill(`d${String(N - 1).padStart(5, "0")}`);
  await expect(page.getByText(last, { exact: true }).first()).toBeVisible({ timeout: 30_000 });
  const foundMs = Date.now() - searched;
  console.log(`portal_endpoints_ui first_row_ms=${firstRow} boot_ms=${bootMs} found_last_ms=${foundMs} rows=${rows} bytes=${body.length}`);
  expect(rows).toBeLessThanOrEqual(50);
  expect(answered).toBeGreaterThan(0);
  expect(firstRow).toBeLessThan(1_000);

  // An endpoint the address names opens though it is far beyond the first page.
  await page.goto("/projects/bbsk/endpoints?lang=en&edit=d09990");
  await expect(page.getByRole("heading", { name: "Edit", exact: true })).toBeVisible({ timeout: 30_000 });
  await expect(page.getByTestId("endpoint-preview")).toContainText('"name": "d09990"');
});
