import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// The cell inspector in the explorer (T-5528): what a person may do with each attribute, as the
// Endpoint decides it, and why. `vite preview` has no gateway, so its answers are stubbed in the
// browser; the explorer, the grid and the panel are the real thing.

const SLUG = "scsd2eehkx42n53z2zyd6vshfh7s7irf";
const TYPE = "BikeHireDockingStation";
const ID = `urn:ngsi-ld:${TYPE}:hel.fi:mobility:001`;
const SHOTS = process.env.SHOTS_DIR ?? "test-results/cell-inspector";

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const manifest = (kind: string, name: string, spec: unknown) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: "helsinki", title: { en: name } },
  spec,
  status: { phase: "Live" },
});

const LINKML = [
  "id: https://hel.fi/models/mobility",
  "name: helsinki-mobility",
  "classes:",
  `  ${TYPE}:`,
  "    slots: [id, name, availableBikeNumber, status, totalSlotNumber]",
  "slots:",
  "  id: {}",
  "  name: { range: string }",
  "  availableBikeNumber: { range: integer }",
  "  status: { range: string }",
  "  totalSlotNumber: { range: integer, annotations: { formula: 'availableBikeNumber + 10' } }",
  "",
].join("\n");

type Persona = "steward" | "editor" | "outsider";

/** Each persona's decisions, as the gateway's `access/attributes` answers them. */
function decisions(persona: Persona) {
  const row = (name: string, read: boolean, write: boolean, reason: string) => ({ name, read, write, delete: false, reason });
  if (persona === "outsider") {
    return ["name", "availableBikeNumber", "status", "totalSlotNumber"].map((name) => row(name, false, false, "no_grant"));
  }
  return [
    row("name", true, true, "grant"),
    row("availableBikeNumber", true, false, "not_in_grant"),
    row("status", true, false, "prohibited"),
    row("totalSlotNumber", true, false, "formula"),
  ];
}

async function stub(page: Page, persona: Persona, access: "ok" | "slow" | "502" = "ok"): Promise<void> {
  await page.route("**/{api,cs}/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s1", username: `${persona}@hel.fi`, name: persona, roles: [] });
    if (path.endsWith(`/api/endpoint/${SLUG}/access/attributes`)) {
      if (access === "slow") return; // never answered: the panel says it is checking
      if (access === "502") return json({ title: "Bad Gateway", status: 502 }, 502);
      return json({ type: TYPE, attributes: decisions(persona) });
    }
    if (path.endsWith("/endpoints/helsinki-bikes/access/attributes")) {
      return json(
        persona === "steward"
          ? { type: TYPE, attribute: "status", action: "write", decision: false, reason: "prohibited", policy: "status-frozen", policyTitle: "Status is set by the operator", policyHidden: false }
          : { type: TYPE, attribute: "status", action: "write", decision: null, reason: null, policy: null, policyTitle: null, policyHidden: true },
      );
    }
    if (path.endsWith("/access/check")) return json({ decision: persona !== "outsider" });
    if (path.endsWith("/access/v1/evaluations")) return json({ evaluations: (JSON.parse(route.request().postData() ?? "{}").evaluations as unknown[]).map(() => ({ decision: persona !== "outsider" })) });
    if (path.endsWith("/access")) return json({ permissions: [{ resource: { type: TYPE }, actions: ["queryEntity"], attributes: "*" }], prohibitions: [] });
    if (path.endsWith("/entities")) {
      const entity = {
        id: ID,
        type: TYPE,
        name: { type: "Property", value: "Rautatientori" },
        availableBikeNumber: { type: "Property", value: 4 },
        status: { type: "Property", value: "working" },
        totalSlotNumber: { type: "Property", value: 14 },
      };
      return route.fulfill({ status: 200, contentType: "application/json", headers: { "NGSILD-Results-Count": "1" }, body: JSON.stringify([entity]) });
    }
    if (path === "/api/v1/projects") return json({ kind: "ProjectList", items: [{ name: "helsinki" }] });
    if (path.endsWith("/spaces")) return json(list([manifest("ContextSpace", "mobility", { dataModelRef: "helsinki-mobility" })]));
    if (path.endsWith("/endpoints")) return json(list([manifest("Endpoint", "helsinki-bikes", { contextSpaceRef: "mobility", slug: SLUG, audience: "project-list" })]));
    if (path.endsWith("/datamodels")) return json(list([manifest("DataModel", "helsinki-mobility", { version: "1.0.0", classes: [TYPE], linkml: LINKML })]));
    return json(list([]));
  });
}

const EXPLORE = `/projects/helsinki/explore?endpoint=helsinki-bikes&type=${TYPE}&attrs=name,availableBikeNumber,status,totalSlotNumber&lang=en`;

async function open(page: Page, attr: string): Promise<void> {
  await page.goto(EXPLORE);
  await expect(page.getByRole("grid")).toBeVisible();
  const button = page.getByRole("button", { name: `Your access: ${attr}` }).first();
  // At 400 px the pinned id column covers the cells at the left edge: bring this one to the middle.
  await button.evaluate((element) => element.scrollIntoView({ block: "center", inline: "center" }));
  await button.click();
  await expect(page.getByRole("dialog", { name: new RegExp(`Your access: ${attr}`) })).toBeVisible();
}

/** `onPage`: the grid holds its rows until the decisions are in, so the state shows on the page, not in a cell's panel. */
const CASES: { name: string; persona: Persona; attr: string; access?: "ok" | "slow" | "502"; says: RegExp; onPage?: boolean }[] = [
  { name: "granted", persona: "steward", attr: "name", says: /Your access allows this/ },
  { name: "read-only", persona: "steward", attr: "availableBikeNumber", says: /Not in your access/ },
  { name: "prohibited-named", persona: "steward", attr: "status", says: /policy “Status is set by the operator”/ },
  { name: "prohibited-hidden", persona: "editor", attr: "status", says: /Hidden by a policy you cannot read/ },
  { name: "formula", persona: "steward", attr: "totalSlotNumber", says: /Calculated by a pipeline/ },
  { name: "loading", persona: "steward", attr: "name", access: "slow", says: /Checking your access/, onPage: true },
  { name: "gateway-502", persona: "steward", attr: "name", access: "502", says: /could not be read/ },
];

for (const width of [1440, 400]) {
  test.describe(`the cell inspector at ${width} px`, () => {
    test.use({ viewport: { width, height: width === 400 ? 860 : 900 } });
    for (const one of CASES) {
      test(one.name, async ({ page }) => {
        await stub(page, one.persona, one.access);
        if (one.onPage) {
          await page.goto(EXPLORE);
          const state = one.access === "slow" ? page.getByRole("status").filter({ hasText: one.says }) : page.getByRole("gridcell", { name: one.says });
          await expect(state.first()).toBeVisible();
          await expect(page.getByRole("button", { name: `Your access: ${one.attr}` })).toHaveCount(0);
        } else {
          await open(page, one.attr);
          const panel = page.getByRole("dialog", { name: new RegExp(`Your access: ${one.attr}`) });
          await expect(panel).toContainText(one.says);
        }
        await page.screenshot({ path: `${SHOTS}/${one.name}-${width}.png` });
      });
    }
    // Nothing readable (T-6210): no attribute column at all, only what every entity has.
    test("no-access", async ({ page }) => {
      await stub(page, "outsider");
      await page.goto(EXPLORE);
      await expect(page.getByRole("columnheader", { name: /ID/ }).first()).toBeVisible();
      await expect(page.getByRole("columnheader", { name: /name/i })).toHaveCount(0);
      await expect(page.getByText("hidden: no access")).toHaveCount(0);
      await page.screenshot({ path: `${SHOTS}/no-access-${width}.png` });
    });
  });
}

test("a write the gateway refuses opens no editor, and the keyboard opens and closes the panel", async ({ page }) => {
  await stub(page, "steward");
  await page.goto(EXPLORE);
  await expect(page.getByRole("textbox", { name: /name/ }).first()).toBeVisible();
  // The prohibited and the read-only values are values, never inputs.
  await expect(page.getByRole("textbox", { name: /status/ })).toHaveCount(0);
  await expect(page.getByRole("textbox", { name: /availableBikeNumber/ })).toHaveCount(0);
  // A click on the cell opens its row on that attribute (owner 2026-11-01); the "i" button opens the access panel.
  const status = page.getByRole("gridcell").filter({ hasText: "working" });
  await status.getByRole("button", { name: "Your access: status" }).click();
  const panel = page.getByRole("dialog", { name: /Your access: status/ });
  await expect(panel).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(panel).toBeHidden();
  await expect(page.getByRole("grid")).toBeFocused();
  await page.keyboard.press("i");
  await expect(panel).toBeVisible();
});
