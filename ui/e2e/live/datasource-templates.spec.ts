/**
 * Every data source template reaches a real public source on dev (T-3249).
 *
 * The steward starts a new data source from each template, gives only what it asks, and the
 * check that runs at once fetches the source on the runner and shows its first records. Nothing
 * is proposed: each form is left after its check, and the drafts the checks kept are swept.
 */
import { expect, test } from "@playwright/test";
import { guideShot } from "./guide";
import { STEWARD, signIn, sweepDrafts } from "./portal";

const SUFFIX = process.env.E2E_SUFFIX ?? new Date().toISOString().slice(11, 16).replace(":", "");
const PROJECT = "helsinki";

const SOURCES: { template: string; url: string; layer?: string }[] = [
  {
    template: "CSV file at an address",
    url: "https://data.smartdublin.ie/dataset/3d3f8721-3b1f-450e-a273-ee750b80dd4c/resource/aee9e89a-e868-417c-b341-7305945702b6/download/dublin-bike-parking.csv",
  },
  { template: "REST API (JSON)", url: "https://api.citybik.es/v2/networks/citybikes-helsinki" },
  {
    template: "Dataset from a CKAN portal",
    url: "https://data.smartdublin.ie/dataset/33ec9fe2-4957-4e9a-ab55-c5e917c7a9ab/resource/2dec86ed-76ed-47a3-ae28-646db5c5b965/download/dublin.csv",
  },
  {
    template: "WFS layer or GeoJSON",
    url: "https://kartta.hel.fi/ws/geoserver/avoindata/wfs",
    layer: "avoindata:Ajoneuvoliikenne_liikennemaarat_viiva",
  },
  { template: "GTFS Realtime feed", url: "https://realtime.hsl.fi/realtime/vehicle-positions/v2/hsl" },
];

test.setTimeout(600_000);

test("each data source template reads its first records from a real source", async ({ browser }) => {
  const { context, page } = await signIn(browser, STEWARD, `/projects/${PROJECT}/datasources?lang=en`);
  try {
    for (const [at, source] of SOURCES.entries()) {
      await page.goto(`/projects/${PROJECT}/datasources?lang=en`, { waitUntil: "load" });
      await page.getByRole("button", { name: "New data source" }).first().click();
      const gallery = page.getByTestId("datasource-templates");
      // Closed at first (T-6163): its summary opens it.
      await gallery.getByText("Start from a template").click();
      // The guide walks the first template; the others only prove their sources answer.
      const shot = (name: string) => (at === 0 ? guideShot(page, name) : Promise.resolve());
      await shot("datasource-1-templates");
      await gallery.getByRole("button", { name: new RegExp(source.template) }).click();
      await gallery.getByLabel(/^(Address|Download address)/).fill(source.url);
      if (source.layer) await gallery.getByLabel("Layer name").fill(source.layer);
      await gallery.getByLabel(/^Name/).fill(`t3249-${at}-${SUFFIX}`);
      await shot("datasource-2-address");
      await gallery.getByRole("button", { name: "Fill the form and check" }).click();
      const probe = page.getByTestId("datasource-probe");
      await expect(probe, `${source.template} answers`).toContainText(/[1-9][0-9]* records/, { timeout: 120_000 });
      await shot("datasource-3-first-records");
    }
  } finally {
    await sweepDrafts(context, page, PROJECT, /^t3249-/);
    await context.close();
  }
});
