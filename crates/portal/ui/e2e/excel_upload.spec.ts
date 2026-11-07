/**
 * Excel at every door (JC1-07), in a browser: the Upload page, Complete this space and the
 * assistant's file question each read a .xlsx in the page, offer its sheets, and hand the chosen
 * sheet on as CSV text. A damaged workbook is said as one and nothing is sent.
 *
 * Run with SHOTS_DIR set to keep the 1440 and 400 pictures of each state.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { axeViolations } from "./axe";
import { workbook } from "../tests/xlsxFixture";

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", name: "Jana Kováčová", email: "jana.kovacova@hel.fi", roles: ["portal-editor"] };
const list = (items: unknown[]) => ({ apiVersion: API, kind: "List", items });

const BOOK = {
  name: "city.xlsx",
  mimeType: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  buffer: Buffer.from(
    workbook({
      Notes: [["author", "remark"], ["Aino", "draft"]],
      Garages: [
        ["name", "totalSpotNumber", "availableSpotNumber", "lat", "lon", "opened"],
        ["Kamppi", 500, 120, 60.17, 24.93, { date: 46086 }],
        ["Kluuvi", 420, 80, 60.171, 24.946, { date: 45900 }],
      ],
    }),
  ),
};
const BROKEN = { name: "broken.xlsx", mimeType: BOOK.mimeType, buffer: Buffer.from("not a zip") };

const CATALOGUE = {
  subjects: [{ name: "dataModel.Parking", models: [{ id: "dataModel.Parking/OffStreetParking", name: "OffStreetParking", attributes: ["name", "location", "totalSpotNumber", "availableSpotNumber"] }] }],
};

const RUN = {
  id: "01J8ZQ4T7K9M2N3P4Q5R6S7T8W",
  project: PROJECT,
  appName: "garages",
  endpointName: "garages",
  appClass: "fullstack",
  visibility: "project",
  prompt: "Build me a map of the garages",
  status: "interviewing",
  steps: 1,
  tokensUsed: 600,
  createdBy: "jana.kovacova",
  createdAt: "2026-11-01T08:00:00Z",
};
const QUESTION = {
  seq: 1,
  questionId: "q-source",
  schema: { type: "object", title: "Where can I read the garages?", properties: { answer: { type: "string", title: "Where can I read the garages?" } } },
  input: { file: { accept: ["csv", "tsv", "xlsx", "json"], maxBytes: 262144 } },
};

async function stub(page: Page, sent: { path: string; body: unknown }[]): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (request.method() !== "GET") {
      sent.push({ path, body: request.postDataJSON() });
      if (path.endsWith("/ops/jc_space_complete")) {
        return json({
          space: "garages",
          found: [],
          drafts: [{ kind: "DataModel", name: "garages", inferred: true, manifest: { kind: "DataModel", metadata: { name: "garages" }, spec: {} }, verdict: { ok: true, findings: [], inputDigest: "a" } }],
          proposeReady: true,
          lane: "yellow",
          change: null,
        });
      }
      if (path.endsWith("/answers")) return route.fulfill({ status: 204 });
      return json({ type: "about:blank", title: "Not stubbed", status: 404 }, 404);
    }
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path === "/api/v1/projects") return json({ ...list([{ name: PROJECT }]), kind: "ProjectList" });
    if (path.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
    if (path === "/api/v1/tools/sdm-catalog") return json(CATALOGUE);
    if (path.endsWith(`/agent-runs/${RUN.id}/events`)) {
      return route.fulfill({ status: 200, contentType: "text/event-stream", headers: { "cache-control": "no-cache" }, body: `id: 1\nevent: question\ndata: ${JSON.stringify(QUESTION)}\n\n` });
    }
    if (path.endsWith(`/agent-runs/${RUN.id}`)) return json(RUN);
    if (path.endsWith("/agent-runs")) return json(list([RUN]));
    if (/^\/api\/v1\/projects\/[a-z-]+\/[a-z-]+$/.test(path)) return json(list([]));
    return json({ type: "about:blank", title: "Not stubbed", status: 404 }, 404);
  });
}

const shot = async (page: Page, name: string) => {
  if (process.env.SHOTS_DIR) await page.screenshot({ path: `${process.env.SHOTS_DIR}/${name}.png`, fullPage: true });
};

for (const [label, width, height] of [["1440", 1440, 900], ["400", 400, 860]] as const) {
  test.describe(`at ${label} px`, () => {
    test.use({ viewport: { width, height } });

    test("Upload offers every sheet of a workbook and proposes the chosen one", async ({ page }) => {
      const sent: { path: string; body: unknown }[] = [];
      await stub(page, sent);
      await page.goto(`/projects/${PROJECT}/spaces/upload?lang=en`);
      await expect(page.getByRole("heading", { level: 1, name: "Upload a CSV" })).toBeVisible();
      await shot(page, `upload-empty-${label}`);
      await page.getByLabel("Choose a CSV file or an Excel workbook (.xlsx)").setInputFiles(BROKEN);
      await expect(page.getByRole("alert")).toContainText("not an Excel workbook");
      await shot(page, `upload-refused-${label}`);
      await page.getByLabel("Choose a CSV file or an Excel workbook (.xlsx)").setInputFiles(BOOK);
      const sheet = page.getByRole("combobox", { name: "Sheet" });
      await sheet.selectOption("Garages");
      await expect(page.getByRole("radio", { name: /OffStreetParking/ })).toBeChecked();
      await shot(page, `upload-sheet-${label}`);
      expect(await axeViolations(page)).toEqual([]);
      expect(sent).toEqual([]);
    });

    test("Complete this space sends the chosen sheet as CSV", async ({ page }) => {
      const sent: { path: string; body: unknown }[] = [];
      await stub(page, sent);
      await page.goto(`/projects/${PROJECT}/spaces/complete?lang=en`);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await shot(page, `complete-empty-${label}`);
      await page.getByLabel("Files", { exact: true }).setInputFiles(BOOK);
      await page.getByRole("combobox", { name: "Sheet" }).selectOption("Garages");
      await shot(page, `complete-sheet-${label}`);
      await page.getByRole("button", { name: "Complete", exact: true }).click();
      await expect(page.getByTestId("complete-draft-DataModel")).toBeVisible();
      const files = (sent.find((s) => s.path.endsWith("/ops/jc_space_complete"))?.body as { files: { name: string; content: string }[] }).files;
      expect(files[0]?.name).toBe("Garages.csv");
      expect(files[0]?.content.split("\r\n")[1]).toBe("Kamppi,500,120,60.17,24.93,2026-03-05");
      await shot(page, `complete-drafted-${label}`);
      expect(await axeViolations(page)).toEqual([]);
    });

    test("a failed check names what to fix and offers no proposal", async ({ page }) => {
      await stub(page, []);
      await page.route("**/ops/jc_space_complete", (route) =>
        route.fulfill({
          contentType: "application/json",
          body: JSON.stringify({
            space: "garages",
            found: [],
            drafts: [{ kind: "DataModel", name: "garages", inferred: true, manifest: { kind: "DataModel", metadata: { name: "garages" }, spec: {} }, verdict: { ok: false, findings: [{ level: "error", path: "linkml", message: "the sample has no header row" }], inputDigest: "a" } }],
            proposeReady: false,
            lane: "yellow",
            change: null,
          }),
        }),
      );
      await page.goto(`/projects/${PROJECT}/spaces/complete?lang=en`);
      await page.getByLabel("Files", { exact: true }).setInputFiles(BOOK);
      await page.getByRole("button", { name: "Complete", exact: true }).click();
      await expect(page.getByTestId("complete-draft-DataModel")).toContainText("Check failed");
      await expect(page.getByTestId("complete-draft-DataModel")).toContainText("no header row");
      await expect(page.getByRole("button", { name: "Propose all" })).toHaveCount(0);
      await shot(page, `complete-failed-check-${label}`);
    });

    test("the assistant's file question takes a workbook and asks which sheet", async ({ page }) => {
      const sent: { path: string; body: unknown }[] = [];
      await stub(page, sent);
      await page.goto(`/projects/${PROJECT}/apps/${RUN.appName}?lang=en`, { waitUntil: "networkidle" });
      const bubble = page.getByRole("button", { name: "Open the assistant" });
      if (await bubble.count()) await bubble.first().click();
      await expect(page.getByText(/csv, tsv, xlsx, json/)).toBeVisible();
      await shot(page, `question-empty-${label}`);
      await page.getByLabel("Choose a file of your data").setInputFiles(BOOK);
      await page.getByRole("combobox", { name: "Sheet" }).selectOption("Garages");
      await shot(page, `question-sheet-${label}`);
      await page.getByRole("button", { name: "Hand over the sheet Garages" }).click();
      await expect.poll(() => sent.find((s) => s.path.endsWith("/answers"))?.body).toMatchObject({
        questionId: "q-source",
        answers: { file: { name: "Garages.csv", format: "csv" } },
      });
      expect(await axeViolations(page)).toEqual([]);
    });
  });
}
