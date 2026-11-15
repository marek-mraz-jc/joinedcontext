import { expect, test } from "@playwright/test";
import { STATIONS } from "../src/fixtures/stations";
import { BASE, serve } from "./serve";

// AP-07, AP-14: the served bundle lists the five stations and the filter hides the ones with none.
test("the stations page shows the five stations and the filter hides the empty ones", async ({ page }) => {
  const { outside, missing, problems } = await serve(page);

  await page.goto(`${BASE}#stations`);
  const stations = page.getByRole("region", { name: "Stations" });
  const table = stations.getByRole("table");
  for (const station of STATIONS) await expect(table.getByText(String(station.name))).toBeVisible();

  await stations.getByRole("checkbox", { name: "Only stations with bikes" }).check();
  await expect(table.getByText("Laivasillankatu")).toHaveCount(0);
  await expect(table.getByText("Sepänkatu")).toHaveCount(0);
  await expect(table.getByText("Viiskulma")).toHaveCount(0);
  await expect(table.getByText("Kaivopuisto")).toBeVisible();

  // AP-11: nothing left the page for another host, and the bundle ran without an error. The
  // map's worker is part of the bundle: a published app has no library folder to find it in.
  await expect(stations.getByTestId("jc-map").locator("canvas")).toHaveCount(1);
  expect(missing).toEqual([]);
  expect(outside).toEqual([]);
  expect(problems).toEqual([]);
});
