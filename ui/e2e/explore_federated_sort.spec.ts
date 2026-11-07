import { mkdirSync } from "node:fs";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";

// T-5618: a space that also reads other brokers refuses `orderBy` (CIM 009 5.7.2.4, 4.23.1). A sort
// on Explore then shows the loaded rows sorted with a line saying so, not the broker's sentence, and
// the remembered sort reopens with rows. `vite preview` has no API, so the browser answers it.

const TYPE = "AirQualityObserved";
const OWN = { name: "helsinki-air-quality", slug: "jtjnhkaau64zmzz5csmcwem52icvufy2" };
const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const LINKML = [
  "id: https://hel.fi/models/air",
  "name: helsinki-air",
  "classes:",
  `  ${TYPE}:`,
  "    slots: [id, name, pm10]",
  "slots:",
  "  id: {}",
  "  name: { range: string }",
  "  pm10: { range: float }",
  "",
].join("\n");
const PM10 = [31, 12, 47, 8];

async function stub(page: Page, refuse: "orderBy" | "any"): Promise<string[]> {
  const reads: string[] = [];
  await page.route("**/{api,cs}/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown, headers: Record<string, string> = {}, status = 200) =>
      route.fulfill({ status, contentType: "application/json", headers, body: JSON.stringify(body) });
    const path = url.pathname;
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: "demo.viewer", name: "Demo Viewer", roles: [], groups: ["helsinki"] });
    if (path.endsWith("/access/check")) return json({ decision: /"(query|retrieve)/.test(route.request().postData() ?? "") });
    if (path.endsWith("/access/v1/evaluations")) return json({ evaluations: (JSON.parse(route.request().postData() ?? "{}").evaluations as { action: { name: string } }[]).map((item) => ({ decision: /^(query|retrieve)/.test(item.action.name) })) });
    if (path.endsWith("/access")) return json({ permissions: [{ resource: { type: TYPE }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }] });
    if (path.endsWith("/entities")) {
      reads.push(url.search);
      if (refuse === "any") {
        return json({ type: "https://uri.etsi.org/ngsi-ld/errors/InvalidRequest", title: "InvalidRequest", detail: "q: unexpected token at 7 (4.9)" }, {}, 400);
      }
      if (url.searchParams.has("orderBy")) {
        return json(
          { type: "https://uri.etsi.org/ngsi-ld/errors/BadRequestData", title: "BadRequestData", detail: "orderBy requires local scope — ordering is never applied to distributed operations (5.7.2.4, 4.23.1)" },
          {},
          400,
        );
      }
      const rows = PM10.map((pm10, n) => ({ id: `urn:ngsi-ld:${TYPE}:hel.fi:helsinki:fmi-${n + 1}`, type: TYPE, name: { type: "Property", value: `Station ${n + 1}` }, pm10: { type: "Property", value: pm10 } }));
      return json(rows, { "NGSILD-Results-Count": String(PM10.length) });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/spaces")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "helsinki", namespace: "helsinki", title: { en: "Helsinki city context" } }, spec: { dataModelRef: "helsinki-air" }, status: { phase: "Live" } }]));
    }
    if (path.endsWith("/endpoints")) {
      return json(list([{
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Endpoint",
        metadata: { name: OWN.name, namespace: "helsinki", title: { en: "Helsinki air quality" } },
        spec: { contextSpaceRef: "helsinki", slug: OWN.slug, enabledRepresentations: ["ngsi-ld"], audience: "public", policyRef: "urn:ngsi-ld:Policy:hel.fi:helsinki:public-air-quality" },
        status: { phase: "Live" },
      }]));
    }
    if (path.endsWith("/datamodels")) {
      return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "DataModel", metadata: { name: "helsinki-air", namespace: "helsinki" }, spec: { version: "1.0.0", classes: [TYPE], linkml: LINKML } }]));
    }
    return json(list([]));
  });
  return reads;
}

const SHOTS = process.env.T5618_SHOTS ?? "test-results/explore-federated-sort";
mkdirSync(SHOTS, { recursive: true });
const ADDRESS = `/projects/helsinki/explore?space=helsinki&endpoint=${OWN.name}&type=${TYPE}&lang=en`;
const pm10Column = async (page: Page) => {
  const header = page.locator("thead th").filter({ hasText: /pm10/i }).first();
  const index = await header.evaluate((th) => Array.from(th.parentElement?.children ?? []).indexOf(th));
  return page.locator("tbody tr").evaluateAll((rows, i) => rows.map((row) => Number(row.children[i]?.textContent?.replace(/[^\d.]/g, ""))), index);
};

for (const width of [1440, 400]) {
  test(`a sort on a space that reads other brokers sorts the loaded rows and says so at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const reads = await stub(page, "orderBy");
    await page.goto(ADDRESS);
    await expect(page.locator("tbody tr")).toHaveCount(PM10.length);

    await page.getByRole("button", { name: /^Sort all rows by pm10/i }).first().click();
    const note = page.getByText(`Sorted within the ${PM10.length} loaded rows: this view also reads other brokers, which do not sort.`);
    await expect(note).toBeVisible();
    await expect(page.getByText(/local scope/)).toHaveCount(0);
    expect(await pm10Column(page)).toEqual([...PM10].sort((a, b) => a - b));
    expect(reads.filter((search) => search.includes("orderBy=")).length).toBe(1);
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: `${SHOTS}/sorted-${width}.png`, fullPage: true });

    // The grid remembers the sort per browser: the next visit opens with rows, sorted, and the note.
    await page.reload();
    await expect(page.locator("tbody tr")).toHaveCount(PM10.length);
    await expect(note).toBeVisible();
    expect(await pm10Column(page)).toEqual([...PM10].sort((a, b) => a - b));
    await page.screenshot({ path: `${SHOTS}/reopened-${width}.png`, fullPage: true });
  });

  test(`another refused query reads in plain words with the broker's sentence folded at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await stub(page, "any");
    await page.goto(ADDRESS);
    await expect(page.getByText("The broker refused this query: InvalidRequest")).toBeVisible();
    await expect(page.getByText("q: unexpected token at 7 (4.9)")).toBeHidden();
    await page.getByText("What the broker said").click();
    await expect(page.getByText("q: unexpected token at 7 (4.9)")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
    await page.screenshot({ path: `${SHOTS}/refused-${width}.png`, fullPage: true });
  });
}
