import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { greenVerdict, isCheck } from "./verdict";
import { axeViolations } from "./axe";

// `vite preview` has no portal API behind it, so the API is answered in the browser and
// everything above it — the project list, the form, the write, the second project's view —
// is the real thing. The stub keeps state: what helsinki proposes, espoo sees (T-0498).

const API = "joinedcontext.com/v1alpha1";
const BIKES_SLUG = "abcdefghijklmnopqrstuvwxyz";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@hel.fi",
  roles: ["portal-editor"],
};

interface Envelope {
  apiVersion: string;
  kind: string;
  metadata: { name: string; namespace: string; labels?: Record<string, string> };
  spec: Record<string, unknown>;
  status?: Record<string, unknown>;
}

function list(items: Envelope[]) {
  return { apiVersion: API, kind: "List", items };
}

function change(namespace: string) {
  return {
    apiVersion: API,
    kind: "Change",
    metadata: { name: "chg-77aa11bb", namespace },
    status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1 } },
  };
}

async function stubApi(page: Page): Promise<{ writes: string[] }> {
  const writes: string[] = [];
  const state: Record<string, Envelope[]> = {
    "/api/v1/projects/helsinki/spaces": [
      { apiVersion: API, kind: "ContextSpace", metadata: { name: "liikenne", namespace: "helsinki" }, spec: {} },
    ],
    "/api/v1/projects/helsinki/endpoints": [
      {
        apiVersion: API,
        kind: "Endpoint",
        metadata: {
          name: "bikes",
          namespace: "helsinki",
          labels: { "joinedcontext.com/space": "liikenne" },
        },
        spec: {
          contextSpaceRef: "liikenne",
          slug: BIKES_SLUG,
          audience: "project-list",
          allowedProjects: [],
          enabledRepresentations: ["ngsi-ld", "geojson"],
        },
        status: { phase: "Live" },
      },
    ],
    "/api/v1/projects/espoo/endpoints": [],
    "/api/v1/projects/espoo/shared": [],
  };

  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path === "/api/v1/projects") {
      return json({ apiVersion: API, kind: "List", items: [{ name: "helsinki" }, { name: "espoo" }] });
    }
    // The Check is a dry run; it answers a verdict and is not one of the writes (PF-57).
    if (isCheck(request.method(), request.url())) {
      return json(greenVerdict(request.postData()));
    }
    const draft = /^\/api\/v1\/projects\/([^/]+)\/drafts\/([^/]+)\/([^/]+)$/.exec(path);
    if (draft && request.method() === "PUT") {
      // The form saves its draft before it proposes (T-0769); the draft is not the change.
      const { manifest } = JSON.parse(request.postData() ?? "{}") as { manifest: unknown };
      const [, project, kind, name] = draft;
      return json({
        project,
        kind,
        name,
        manifest,
        touchedBy: "steward",
        touchedKind: "person",
        version: 1,
        updatedAt: new Date().toISOString(),
      });
    }
    if (request.method() !== "GET") {
      const body = JSON.parse(request.postData() ?? "{}") as Envelope;
      writes.push(`${request.method()} ${path} ${request.postData() ?? ""}`);
      // The approvals flow applies the change; here it lands at once so the next view sees it.
      const collection = request.method() === "POST" ? path : path.slice(0, path.lastIndexOf("/"));
      const items = state[collection] ?? [];
      state[collection] = [...items.filter((item) => item.metadata.name !== body.metadata.name), body];
      return json(change(body.metadata.namespace), 202);
    }
    return json(list(state[path] ?? []));
  });
  return { writes };
}

test.describe("sharing", () => {
  test("shares an endpoint with a second project, which references it (EP-14, EP-15)", async ({
    page,
  }) => {
    const { writes } = await stubApi(page);

    // helsinki opens the bikes endpoint to espoo: a checkbox from the project list, not a field.
    await page.goto("/projects/helsinki/endpoints?lang=en");
    const row = page.getByRole("row").filter({ hasText: "bikes" });
    // The row keeps what it answers in the open and the rest behind its menu (T-2287).
    await row.getByRole("button", { name: /More actions/ }).click();
    await page.getByRole("menuitem", { name: "Edit" }).click();
    const dialog = page.getByTestId("form-page");
    await dialog.getByRole("checkbox", { name: "espoo" }).check();
    await expect(dialog.getByRole("checkbox", { name: "helsinki" })).toHaveCount(0);
    // Strict validation proposes nothing without a fresh green verdict (T-0779).
    await dialog.getByRole("button", { name: "Check", exact: true }).click();
    await expect(dialog.getByTestId("draft-verdict")).toContainText("Checked");
    await dialog.getByRole("button", { name: "Propose change" }).click();

    await expect(page.getByText("chg-77aa11bb")).toBeVisible();
    expect(writes).toHaveLength(1);
    expect(writes[0]).toContain("PUT /api/v1/projects/helsinki/endpoints/bikes");
    expect(writes[0]).toContain('"allowedProjects":["espoo"]');
    // The owner sees who it is shared with.
    await expect(row.getByRole("list", { name: "Shared with" }).getByText("espoo")).toBeVisible();

    // espoo sees it as a row of its Endpoints grid (T-6169) and references it from the row menu.
    await page.goto("/projects/espoo/endpoints?lang=en");
    const shared = page.getByRole("table", { name: "Endpoints" });
    const bikes = shared.getByRole("row").filter({ hasText: "bikes" });
    await expect(bikes.getByText("Shared by helsinki")).toBeVisible();
    await bikes.getByRole("button", { name: /More actions/ }).click();
    await page.getByRole("menuitem", { name: "Use in this project" }).click();

    await expect(page.getByText("chg-77aa11bb")).toBeVisible();
    expect(writes).toHaveLength(2);
    expect(writes[1]).toContain("POST /api/v1/projects/espoo/shared");
    expect(writes[1]).toContain('"kind":"SharedSpaceReference"');
    // A reference names the endpoint by project and name, never by its slug (EP-77, CC-83).
    expect(writes[1]).toContain('"endpointRef":{"project":"helsinki","name":"bikes"}');
    expect(writes[1]).not.toContain(BIKES_SLUG);
    expect(writes[1]).toContain('"alias":"helsinki-liikenne"');
    // Declared: the row says Referenced and its menu offers Stop using instead of Use.
    await expect(bikes.getByText("Referenced")).toBeVisible();
    await bikes.getByRole("button", { name: /More actions/ }).click();
    await expect(page.getByRole("menuitem", { name: "Stop using" })).toBeVisible();
    await expect(page.getByRole("menuitem", { name: "Use in this project" })).toHaveCount(0);
    await page.keyboard.press("Escape");

    // The references have no page of their own any more: the sidebar does not offer one, and
    // the old URL lands on this section (T-0706).
    await expect(
      page.getByRole("navigation", { name: "Main navigation" }).getByRole("link", { name: "Shared references" }),
    ).toHaveCount(0);
    await page.goto("/projects/espoo/shared");
    await expect(page.getByRole("table", { name: "Endpoints" })).toBeVisible();
    expect(new URL(page.url()).pathname).toBe("/projects/espoo/endpoints");
  });

  test("the endpoints route has no axe violations, empty and with nothing shared", async ({ page }) => {
    await stubApi(page);

    await page.goto("/projects/espoo/endpoints?lang=sk");
    // Nothing shared with espoo yet: the empty card offers the way to what other projects publish.
    await expect(page.getByRole("link", { name: "Pozrieť, čo zverejňujú iné projekty" })).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
  });
});
