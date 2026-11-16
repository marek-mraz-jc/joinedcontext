import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";
import { manifestReadsAsRest } from "./manifestReads";

// The DataSource doors run through the OData service (T-7117); the stubs answer their REST doors.
test.beforeEach(({ page }) => manifestReadsAsRest(page));

// PL-122: a steward adds a webhook data source, reads where its sender posts and sends a test
// event. `vite preview` has no Portal API behind it, so the browser answers it; the clicks, the
// form and the rendering are the real ones. The runner's own answers (200, 401, 413, 404 after a
// delete) are proven in jc's `a_webhook_stream_answers_only_its_sender_and_goes_with_its_stream`.

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const item = (kind: string, name: string, spec: unknown) => ({
  apiVersion: API,
  kind,
  metadata: { name, namespace: PROJECT },
  spec,
});
const HOOK = item("DataSource", "ttn", {
  type: "webhook",
  webhook: { auth: { hmac: { header: "X-Hub-Signature-256", secretRef: { name: "ttn-webhook", key: "secret" } } } },
});
const LISTS: Record<string, unknown[]> = {
  datasources: [HOOK],
  pipelines: [item("Pipeline", "ttn-uplinks", { class: "resident", source: { dataSourceRef: "ttn" } })],
};

async function stubApi(page: Page, answers: Array<{ status: number; answer: string }>): Promise<unknown[]> {
  const sent: unknown[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "aino.virtanen", name: "Aino Virtanen", roles: ["portal-editor"] });
    }
    // No draft is saved for this source; the form's own draft save is answered as stored.
    if (url.pathname.includes("/drafts")) {
      if (request.method() !== "PUT") return json({ title: "Not Found", status: 404, detail: "no draft" }, 404);
      const { manifest } = request.postDataJSON() as { manifest: unknown };
      return json({ project: PROJECT, kind: "DataSource", name: "ttn", manifest, touchedBy: "b7c1e0f4", touchedKind: "person", version: 1, updatedAt: "2026-11-15T10:00:00Z" });
    }
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: PROJECT }] });
    if (url.pathname.endsWith("/datasources/ttn/webhook")) {
      return json({
        auth: "hmac",
        header: "X-Hub-Signature-256",
        maxBodyBytes: 1048576,
        published: true,
        addresses: [{ pipeline: "ttn-uplinks", url: "https://portal.smartcity.example/ingest/default/helsinki.ttn-uplinks/ttn" }],
      });
    }
    if (url.pathname.endsWith("/datasources/ttn/test-event")) {
      sent.push(request.postDataJSON());
      return json(answers.shift() ?? { status: 200, answer: "" });
    }
    if (url.pathname.endsWith("/datasources/ttn")) return json(HOOK);
    const plural = url.pathname.split("/").pop() ?? "";
    return json({ apiVersion: API, kind: "List", items: LISTS[plural] ?? [] });
  });
  return sent;
}

async function shoot(page: Page, name: string): Promise<void> {
  const shots = process.env.T7088_SHOTS;
  if (!shots) return;
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
    await page.screenshot({ path: `${shots}/${name}-${width}.png`, fullPage: true });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

test.describe("webhook data source", () => {
  test("a steward picks the webhook type and every field says what to enter", async ({ page }) => {
    await stubApi(page, []);
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`/projects/${PROJECT}/datasources?lang=en`);
    await page.getByRole("button", { name: "New data source" }).first().click();
    await page.getByRole("combobox", { name: "Type" }).selectOption("webhook");
    const dialog = page.getByRole("region", { name: "New data source" });
    await expect(dialog.getByText("How the sender proves itself").first()).toBeVisible();
    await expect(dialog.getByText(/1 to 16777216, for example 1048576/)).toBeVisible();
    await expect(dialog.getByText(/TLS certificates/)).toHaveCount(0);
    expect(await axeViolations(page)).toEqual([]);
    await shoot(page, "webhook-new");
  });

  // The page lays a phone out anew, which drops an answer on screen: each width sends its own.
  for (const width of [1440, 400]) {
    test(`a steward reads where the sender posts and sends a test event (${width})`, async ({ page }) => {
      const sent = await stubApi(page, [
        { status: 401, answer: "Unauthorized" },
        { status: 200, answer: "" },
      ]);
      const shots = process.env.T7088_SHOTS;
      const shot = async (name: string) => {
        if (shots) await page.screenshot({ path: `${shots}/${name}-${width}.png`, fullPage: true });
      };
      await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
      await page.goto(`/projects/${PROJECT}/datasources?lang=en`);
      await page.getByRole("link", { name: "ttn", exact: true }).click();
      const panel = page.getByTestId("datasource-webhook");
      await expect(panel.getByText("https://portal.smartcity.example/ingest/default/helsinki.ttn-uplinks/ttn")).toBeVisible();
      await expect(panel.getByText(/Header: X-Hub-Signature-256/)).toBeVisible();
      await expect(page.getByRole("button", { name: "Test connection" })).toHaveCount(0);
      await shot("webhook-address");

      await panel.getByLabel("Body").fill('{"temperature": 21.5}');
      await panel.getByRole("button", { name: "Send test event" }).click();
      await expect(panel.getByText("The runner answered 401: Unauthorized")).toBeVisible();
      await shot("webhook-test-refused");

      await panel.getByRole("button", { name: "Send test event" }).click();
      await expect(panel.getByText(/Accepted \(200\)/)).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await shot("webhook-test-accepted");
      expect(sent).toEqual([
        { pipeline: "ttn-uplinks", body: '{"temperature": 21.5}' },
        { pipeline: "ttn-uplinks", body: '{"temperature": 21.5}' },
      ]);
    });
  }
});
