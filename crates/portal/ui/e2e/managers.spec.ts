import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { greenVerdict, isCheck } from "./verdict";
import { axeViolations } from "./axe";
import sk from "../src/locales/sk.json" with { type: "json" };

// Same shape as the approvals journey: `vite preview` has no portal API behind it, so the API
// is answered in the browser and everything above it — routing, forms, clicks — is the real thing.
// The project is `helsinki`: the root route goes to the first project `GET /api/v1/projects`
// lists (PF-05), so the stub answers it with that one.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const SOURCE_URL =
  "https://git.example.sk/bb/org/src/branch/main/projects/helsinki/spaces/ovzdusie/space.yaml";

const PROJECTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ProjectList",
  items: [{ name: "helsinki" }],
};

const SPACES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSpace",
      metadata: {
        name: "ovzdusie",
        namespace: "helsinki",
        title: { sk: "Ovzdušie", en: "Air quality" },
      },
      spec: { dataModelRef: "AirQualityObserved" },
      status: { phase: "Live", observedRevision: "9c1f0ab", sourceUrl: SOURCE_URL },
    },
  ],
};

const ENDPOINTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: {
        name: "public-air",
        namespace: "helsinki",
        labels: { "joinedcontext.com/space": "ovzdusie" },
      },
      spec: {
        contextSpaceRef: "ovzdusie",
        slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
        audience: "public",
        enabledRepresentations: ["ngsi-ld", "geojson"],
      },
      status: { phase: "Live" },
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "helsinki" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
};

async function stubApi(page: Page): Promise<{ writes: string[] }> {
  const writes: string[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
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
      writes.push(`${request.method()} ${path} ${request.postData() ?? ""}`);
      return json(CHANGE, 202);
    }
    if (path === "/api/v1/projects") {
      return json(PROJECTS);
    }
    if (path.endsWith("/spaces")) {
      return json(SPACES);
    }
    if (path.endsWith("/endpoints")) {
      return json(ENDPOINTS);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return { writes };
}

test.describe("managers", () => {
  test("a space links to its manifest in the forge", async ({ page }) => {
    await stubApi(page);

    // The home is where "/" lands now (T-3233); the spaces are one page further.
    await page.goto("/projects/helsinki/spaces?lang=en");
    await expect(page).toHaveURL(/\/projects\/helsinki\/spaces/);
    await expect(page.getByText("Air quality")).toBeVisible();
    // The manifest in the forge is an item of the row menu now (T-6095), not a column of icons.
    await page.getByRole("button", { name: "More actions for Air quality" }).click();
    await expect(page.getByRole("menuitem", { name: "View source (Git)", exact: true })).toHaveAttribute("href", SOURCE_URL);
  });

  test("adding a representation to an endpoint proposes a change", async ({ page }) => {
    const { writes } = await stubApi(page);

    await page.goto("/projects/helsinki/endpoints?lang=en");
    await expect(page.getByText("public-air")).toBeVisible();

    // The row keeps what it answers in the open and the rest behind its menu (T-2287).
    await page.getByRole("button", { name: /More actions/ }).click();
    await page.getByRole("menuitem", { name: "Edit" }).click();
    const dialog = page.getByTestId("form-page");
    await dialog.getByRole("checkbox", { name: "csv" }).check();
    // Strict validation proposes nothing without a fresh green verdict (T-0779).
    await dialog.getByRole("button", { name: "Check", exact: true }).click();
    await expect(dialog.getByTestId("draft-verdict")).toContainText("Checked");
    await dialog.getByRole("button", { name: "Propose change" }).click();

    await expect(page.getByText("chg-77aa11bb")).toBeVisible();
    await expect(page.getByRole("link", { name: "Review it in Approvals" })).toBeVisible();

    expect(writes).toHaveLength(1);
    expect(writes[0]).toContain("PUT /api/v1/projects/helsinki/endpoints/public-air");
    expect(writes[0]).toContain("csv");
  });

  test("the manager routes have no axe violations", async ({ page }) => {
    await stubApi(page);

    await page.goto("/projects/helsinki/spaces?lang=sk");
    await expect(page.getByRole("table")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);

    await page.goto("/projects/helsinki/endpoints?lang=sk");
    // The route holds two tables since T-0498: the project's own and the shared section's.
    // The Slovak name is the locale's own (T-3278 gave endpoint one Slovak term), never a copy here.
    await expect(page.getByRole("table", { name: sk.endpoints.title })).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
  });
});
