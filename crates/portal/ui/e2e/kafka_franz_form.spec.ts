import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { axeViolations } from "./axe";
import { doorsAsRest } from "./manifestReads";

// The projects list is a Portal door (portal_project_list, T-7177): the stub answers its REST address.
test.beforeEach(({ page }) => doorsAsRest(page, ["portal_project_list"]));

// T-7093: the runner builds kafka_franz, so a steward picks it on a new data source and fills the
// brokers, the topics and the group, each field with its example. `vite preview` has no Portal API
// behind it, so the browser answers it; the pick, the form and its validation are the real ones.

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const SHOTS = process.env.SHOTS_DIR;

async function stubApi(page: Page): Promise<void> {
  await page.route("**/api/v1/**", async (route) => {
    const url = new URL(route.request().url());
    const json = (body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname.endsWith("/auth/me")) {
      return json({ subject: "b7c1e0f4", username: "aino.virtanen", name: "Aino Virtanen", roles: ["portal-approver"] });
    }
    if (url.pathname === "/api/v1/projects") return json({ apiVersion: API, kind: "ProjectList", items: [{ name: PROJECT }] });
    return json({ apiVersion: API, kind: "List", items: [] });
  });
}

async function shot(page: Page, state: string): Promise<void> {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: `${SHOTS}/kafka-${state}-${width}.png`, fullPage: true, animations: "disabled" });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
}

test("a steward picks Kafka and fills the brokers, the topics and the group, each with its example", async ({ page }) => {
  await stubApi(page);
  await page.goto(`/projects/${PROJECT}/datasources/new?lang=en`);
  await page.locator("#datasource-type").selectOption({ label: "Kafka" });
  // the summary reads as words, without Bento's markdown link
  await expect(page.getByTestId("input-summary")).toHaveText("A Kafka input using the Franz Kafka client library.");
  for (const example of ["e.g. kafka-1.example.org:9092", "e.g. vehicle-positions", "e.g. traffic-ingest", "password: ${KAFKA_PASSWORD}"]) {
    await expect(page.getByText(example, { exact: false }).first()).toBeVisible();
  }
  await shot(page, "empty");
  expect(await axeViolations(page)).toEqual([]);

  // proposing with nothing filled names what is missing
  await page.getByRole("button", { name: "Propose change" }).click();
  await expect(page.getByText(/Fields to fix: 3\. First: Name/)).toBeVisible();
  await shot(page, "refused");

  await page.getByRole("textbox", { name: "Name", exact: true }).fill("traffic-kafka");
  // a required list opens with its first field
  await page.locator("#root_seed_brokers_0").fill("kafka-1.example.org:9092");
  await page.locator("#root_topics_0").fill("vehicle-positions");
  await page.getByLabel("Consumer group").fill("traffic-ingest");
  await expect(page.getByText(/Fields to fix/)).toBeHidden();
  // a corrected field drops its error style
  for (const id of ["#root_seed_brokers_0", "#root_topics_0"]) {
    await expect(page.locator(id)).not.toHaveAttribute("aria-invalid", "true");
  }
  await shot(page, "filled");
  expect(await axeViolations(page)).toEqual([]);
});
