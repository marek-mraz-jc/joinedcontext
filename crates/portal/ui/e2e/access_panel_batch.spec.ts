import { mkdirSync } from "node:fs";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-5392: the Access panel asks the Endpoint every action it shows in one AuthZEN
// `access/v1/evaluations` request and shows each decision; a failed batch says so instead of
// "checking…". `vite preview` has no API, so the browser answers it.

const TYPE = "AirQualityObserved";
const OWN = {
  name: "helsinki-air-quality",
  slug: "jtjnhkaau64zmzz5csmcwem52icvufy2",
};
const list = (items: unknown[]) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items,
});
const LINKML = [
  "id: https://hel.fi/models/air",
  "name: helsinki-air",
  "classes:",
  `  ${TYPE}:`,
  "    slots: [id, name]",
  "slots:",
  "  id: {}",
  "  name: { range: string }",
  "",
].join("\n");

type Answer = "editor" | "viewer" | "failing";

async function stub(page: Page, answer: Answer): Promise<string[]> {
  const asked: string[] = [];
  await page.route("**/{api,cs}/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    if (path.endsWith("/auth/me"))
      return json({
        subject: "s1",
        username: "demo.viewer",
        name: "Demo Viewer",
        roles: [],
        groups: ["helsinki"],
      });
    if (
      path.endsWith("/access/v1/evaluations") ||
      path.endsWith("/access/check")
    )
      asked.push(`${path.split("/access")[1]} ${request.postData() ?? ""}`);
    if (path.endsWith("/access/v1/evaluations")) {
      if (answer === "failing")
        return json({ title: "Bad Gateway", status: 502 }, 502);
      const items = (
        JSON.parse(request.postData() ?? "{}") as {
          evaluations: { action: { name: string } }[];
        }
      ).evaluations;
      return json({
        evaluations: items.map((item) => ({
          decision: answer === "editor" || item.action.name === "queryEntity",
        })),
      });
    }
    if (path.endsWith("/access/check")) return json({ decision: false });
    if (path.endsWith("/access"))
      return json({
        permissions: [
          {
            resource: { type: TYPE },
            actions: ["queryEntity", "retrieveEntity"],
            attributes: "*",
          },
        ],
      });
    if (path.endsWith("/entities"))
      return json(
        [1, 2].map((n) => ({
          id: `urn:ngsi-ld:${TYPE}:fmi-${n}`,
          type: TYPE,
          name: `Station ${n}`,
        })),
      );
    if (path === "/api/v1/projects")
      return json({
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "ProjectList",
        items: [{ name: "helsinki" }],
      });
    if (path.endsWith("/spaces")) {
      return json(
        list([
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "ContextSpace",
            metadata: {
              name: "helsinki",
              namespace: "helsinki",
              title: { en: "Helsinki city context" },
            },
            spec: { dataModelRef: "helsinki-air" },
            status: { phase: "Live" },
          },
        ]),
      );
    }
    if (path.endsWith("/endpoints")) {
      return json(
        list([
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Endpoint",
            metadata: {
              name: OWN.name,
              namespace: "helsinki",
              title: { en: "Helsinki air quality" },
            },
            spec: {
              contextSpaceRef: "helsinki",
              slug: OWN.slug,
              enabledRepresentations: ["ngsi-ld"],
              audience: "public",
            },
            status: { phase: "Live" },
          },
        ]),
      );
    }
    if (path.endsWith("/datamodels")) {
      return json(
        list([
          {
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "DataModel",
            metadata: { name: "helsinki-air", namespace: "helsinki" },
            spec: { version: "1.0.0", classes: [TYPE], linkml: LINKML },
          },
        ]),
      );
    }
    return json(list([]));
  });
  return asked;
}

const SHOTS = process.env.T5392_SHOTS ?? "test-results/access-panel-batch";
mkdirSync(SHOTS, { recursive: true });

const EXPECTED: Record<Answer, string[]> = {
  editor: [`You may read ${TYPE}`, `You may change ${TYPE}`],
  viewer: [`You may read ${TYPE}`, `You may not change ${TYPE}`],
  failing: ["These permissions could not be checked: HTTP 502"],
};

for (const width of [1440, 400]) {
  for (const answer of ["editor", "viewer", "failing"] as const) {
    test(`asks every action in one batch and shows the ${answer} decisions at ${width}px`, async ({
      page,
    }) => {
      await page.setViewportSize({ width, height: 900 });
      const asked = await stub(page, answer);
      await page.goto(
        `/projects/helsinki/explore?space=helsinki&type=${TYPE}&lang=en`,
      );
      // The panel sits in the toolbar's "Your access" popover; open it to read the decisions.
      await page.locator("summary", { hasText: "Your access" }).click();
      const panel = page.getByRole("region", { name: "Your access" });
      for (const text of EXPECTED[answer])
        await expect(panel.getByText(text)).toBeVisible();
      // One batch with both actions; no per-action check for the panel's two.
      const batches = asked.filter((line) =>
        line.startsWith("/v1/evaluations"),
      );
      expect(batches).toHaveLength(1);
      expect(batches[0]).toContain('"queryEntity"');
      expect(batches[0]).toContain('"updateAttrs"');
      expect(
        asked.filter(
          (line) =>
            line.startsWith("/check") && /queryEntity|updateAttrs/.test(line),
        ),
      ).toEqual([]);
      await panel.scrollIntoViewIfNeeded();
      await page.screenshot({
        path: `${SHOTS}/${answer}-${width}.png`,
        fullPage: false,
      });
    });
  }
}
