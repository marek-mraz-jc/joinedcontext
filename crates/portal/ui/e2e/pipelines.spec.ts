import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";
import { greenVerdict, isCheck } from "./verdict";

// `vite preview` has no portal API behind it, so the API is answered in the browser and
// everything above it — routing, polling, the pause button — is the real thing.

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const PIPELINES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Pipeline",
      metadata: {
        name: "aq-mqtt-ingest",
        namespace: "banskabystrica",
        title: { sk: "Ovzdušie MQTT", en: "Air quality MQTT" },
      },
      spec: {
        class: "resident",
        secretRefs: [{ name: "mqtt-credentials", key: "password", envVar: "MQTT_PASSWORD" }],
      },
      status: { phase: "Live" },
    },
  ],
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
};

// A merged change that wrote an earlier version of the pipeline (T-3975).
const EARLIER = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000aa01", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "Merged", plan: { create: 0, update: 1, delete: 0 } },
  summary: { key: "change.summary.update", params: { kind: "Pipeline", name: "aq-mqtt-ingest", fields: 1 } },
  author: { name: "Jana Kováčová", email: "jana.kovacova@banskabystrica.sk" },
  createdAt: "2026-10-01T08:00:00Z",
  decision: { by: "peter.approver@banskabystrica.sk", at: "2026-10-01T09:00:00Z" },
};

const EARLIER_MANIFEST = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Pipeline",
  metadata: { name: "aq-mqtt-ingest", namespace: "banskabystrica" },
  spec: {
    class: "resident",
    outputs: [{ targetEndpoint: "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:air", type: "AirQualityObserved", mode: "upsert" }],
    steps: [{ kind: "bloblang", bloblang: "root = this.earlier" }],
    secretRefs: [{ name: "mqtt-credentials", key: "password", envVar: "MQTT_PASSWORD" }],
  },
};

async function stubApi(page: Page): Promise<{ writes: string[] }> {
  const writes: string[] = [];
  const templates: unknown[] = [];
  let received = 128_400;
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    // Pause checks the manifest it is about to write, and a check is a dry run whatever verb it
    // uses: a `PUT …?dryRun=All` is not a write (PF-57, T-2264).
    if (isCheck(request.method(), request.url())) {
      return json(greenVerdict(request.postData()));
    }
    // The project's templates live in the Portal's own store (T-3976).
    if (path.endsWith("/pipelinetemplates")) {
      if (request.method() === "POST") {
        const body = JSON.parse(request.postData() ?? "{}") as { title: string; manifest: { spec: Record<string, unknown> } };
        writes.push(`POST ${path} ${request.postData() ?? ""}`);
        const { secretRefs: _refs, ...spec } = body.manifest.spec;
        void _refs;
        templates.unshift({ id: "3c0f6f0e-8a1b-4c2d-9e4f-5a6b7c8d9e0f", title: body.title, spec, createdBy: IDENTITY.name, createdAt: new Date().toISOString() });
        return json(templates[0], 201);
      }
      return json({ items: templates });
    }
    if (request.method() !== "GET") {
      writes.push(`${request.method()} ${path} ${request.postData() ?? ""}`);
      return json(CHANGE, 202);
    }
    if (path.endsWith("/metrics")) {
      // A counter that grows between polls, the way a running stream's does.
      received += 25;
      return json({
        pipeline: "aq-mqtt-ingest",
        scrapedAt: new Date().toISOString(),
        received,
        sent: received - 3,
        errors: 3,
        bufferDepth: 340,
      });
    }
    if (path.endsWith("/changes/history")) {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ChangeList", items: [EARLIER] });
    }
    if (path.endsWith(`/changes/${EARLIER.metadata.name}/manifests`)) {
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", metadata: {}, items: [EARLIER_MANIFEST] });
    }
    if (path.endsWith("/pipelines")) {
      return json(PIPELINES);
    }
    if (path.endsWith("/pipelines/aq-mqtt-ingest")) {
      return json(PIPELINES.items[0]);
    }
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return { writes };
}

test.describe("pipelines", () => {
  test("the resident stream's counter moves while the view is open (DEMO step 3)", async ({
    page,
  }) => {
    await stubApi(page);
    await page.clock.install();

    await page.goto("/projects/banskabystrica/pipelines?lang=en");
    const row = page.getByRole("row").filter({ hasText: "Air quality MQTT" });
    await expect(row.getByText("resident")).toBeVisible();
    await expect(row.getByText(/128,425/)).toBeVisible();
    // The row polls the runner every 15 s since T-4817; the number has to change on its own.
    await page.clock.fastForward(15_000);
    await expect(row.getByText(/128,450/)).toBeVisible();
    // The credentials moved from the list to the pipeline's overview, names only (PL-17, T-4817).
    await page.goto("/projects/banskabystrica/pipelines/aq-mqtt-ingest?lang=en");
    await expect(page.getByText("mqtt-credentials/password", { exact: true })).toBeVisible();
  });

  // The buffer depth the metrics route answers is the queue on the wire into the output, amber
  // past the limit, as NiFi shows a connection's queue (T-3973).
  test("the canvas shows the queue before the output", async ({ page }) => {
    await stubApi(page);

    await page.goto("/projects/banskabystrica/pipelines/aq-mqtt-ingest/edit?lang=en");
    const dialog = page.getByTestId("form-page");
    await dialog.getByText("More options").click();
    await dialog.getByText("Advanced editor").click();
    const queue = page.getByTestId("flow-queue");
    await expect(queue).toContainText("340 queued");
    await expect(queue).toHaveAttribute("data-high", "true");
  });

  // The pipeline's own history: a merged change's version goes back into the form and is
  // proposed as a new change after Check, like any edit (T-3975).
  test("restores an earlier version from the pipeline's own history", async ({ page }) => {
    const { writes } = await stubApi(page);

    await page.goto("/projects/banskabystrica/pipelines/aq-mqtt-ingest/edit?lang=en");
    const dialog = page.getByTestId("form-page");
    await dialog.getByRole("tab", { name: "History" }).click();
    const row = dialog.getByTestId("resource-history-item");
    await expect(row).toContainText("chg-0000aa01");
    await row.getByRole("button", { name: "Restore this version" }).click();
    await expect(dialog.getByTestId("history-restored")).toContainText("chg-0000aa01");
    expect(writes).toEqual([]);

    await dialog.getByRole("button", { name: "Check", exact: true }).click();
    await dialog.getByRole("button", { name: "Propose change" }).click();
    await expect(page.getByText("chg-77aa11bb")).toBeVisible();
    const proposed = writes.find((write) => write.includes("/pipelines/aq-mqtt-ingest"));
    expect(proposed, writes.join("\n")).toContain("root = this.earlier");
    expect(proposed).toContain("urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:air");
  });

  // A pipeline saved as a template of the project starts the next one (T-3976).
  test("builds a second pipeline from a template saved from the first", async ({ page }) => {
    const { writes } = await stubApi(page);

    await page.goto("/projects/banskabystrica/pipelines/aq-mqtt-ingest/edit?lang=en");
    const dialog = page.getByTestId("form-page");
    await dialog.getByRole("button", { name: "Save as template" }).click();
    await dialog.getByLabel(/^Template title/).fill("MQTT sensor feed");
    await dialog.getByRole("button", { name: "Save template" }).click();
    await expect(dialog.getByRole("status").filter({ hasText: "Saved as template" })).toContainText("MQTT sensor feed");
    const saved = JSON.parse(writes.find((w) => w.includes("/pipelinetemplates"))!.split(" ").slice(2).join(" ")) as {
      manifest: { metadata: { name: string } };
    };
    expect(saved.manifest.metadata.name).toBe("aq-mqtt-ingest");

    await page.goto("/projects/banskabystrica/pipelines?lang=en");
    await page.getByRole("button", { name: "New pipeline" }).first().click();
    const offered = page.getByTestId("pipeline-template").filter({ hasText: "MQTT sensor feed" });
    await expect(offered).toBeVisible();
    await offered.getByRole("button", { name: "Use" }).click();
    await expect(page.getByTestId("form-page")).toBeVisible();
  });

  test("pausing a pipeline proposes a change", async ({ page }) => {
    const { writes } = await stubApi(page);

    await page.goto("/projects/banskabystrica/pipelines?lang=en");
    await page.getByRole("button", { name: "Pause" }).click();

    await expect(page.getByText("chg-77aa11bb")).toBeVisible();
    expect(writes).toHaveLength(1);
    expect(writes[0]).toContain("PUT /api/v1/projects/banskabystrica/pipelines/aq-mqtt-ingest");
    expect(writes[0]).toContain('"enabled":false');
  });

  test("the pipelines route has no axe violations", async ({ page }) => {
    await stubApi(page);

    await page.goto("/projects/banskabystrica/pipelines?lang=sk");
    await expect(page.getByRole("table")).toBeVisible();
    expect(await axeViolations(page)).toEqual([]);
  });
});
