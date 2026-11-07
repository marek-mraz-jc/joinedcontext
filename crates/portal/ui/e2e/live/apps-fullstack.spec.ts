/**
 * The two Rust + React sample applications on dev, each built from its own repository on the
 * forge into an image the reconciler runs (T-2617, AP-34, AP-40, AP-62, AP-75, AP-105, AP-109,
 * AP-110).
 *
 * The forge bootstrap pushes `apps/hsl-transport` and `apps/air-quality` into `helsinki_<name>`,
 * the rust-1.90 lane builds each image, the Portal publishes it and the reconciler runs it. Here
 * an anonymous visitor watches the buses move; the viewer reads the air-quality stations and is
 * offered no form, and the backend repeats the gateway's 403 when the viewer writes anyway; the
 * steward adds a station, corrects it and removes it, and the viewer sees the change. Dev keeps
 * nothing: the one record has a fixed id and is removed in `finally`.
 *
 * Every wait is on the thing the step needs, never on the network going idle (T-2452).
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { STEWARD, VIEWER, goSignedIn, signIn } from "./portal";

const PROJECT = "helsinki";
/** The origin the apps are served on; the Portal's own host sends `/apps/*` there with a 308. */
const APPS_URL = process.env.APPS_URL ?? "https://dev.joinedcontext.com";
const AIR = `${APPS_URL}/apps/air-quality/`;
/** The one station the steward writes, a fixed id so a run that died leaves one to find. */
const LOCAL_ID = "t2617-journey";
const NAME = "T-2617 journey station";

test.setTimeout(900_000);

interface Station {
  id: string;
  name?: string;
  own?: boolean;
  stewardNote?: string;
}

async function stations(page: Page): Promise<Station[]> {
  const answer = await page.request.get(`${AIR}api/stations`);
  expect(answer.status(), await answer.text()).toBe(200);
  return (await answer.json()) as Station[];
}

// AP-75, AP-105, AP-110: each is served from an image built at its own repository's commit, and
// the catalog says which one.
test("each fullstack sample is built from its own repository into an image, and the catalog says which commit", async ({
  browser,
}) => {
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/apps?lang=en`);
  try {
    for (const app of ["hsl-transport", "air-quality"]) {
      const answer = await viewer.page.request.get(`/api/v1/projects/${PROJECT}/apps/${app}`);
      expect(answer.status(), `${app} is in the project`).toBe(200);
      const manifest = (await answer.json()) as {
        spec: { kind?: string; source?: { git?: { url?: string; ref?: string } } };
        status?: { build?: { commit?: string; digest?: string } };
      };
      expect(manifest.spec.kind).toBe("fullstack");
      expect(manifest.spec.source?.git?.url, `${app} names its own repository`).toContain(`/${PROJECT}_${app}.git`);
      expect(manifest.spec.source?.git?.ref, `${app} is pinned to a commit`).toMatch(/^[0-9a-f]{40}$/);
      expect(manifest.status?.build?.commit, `${app} is built at the commit it names`).toBe(manifest.spec.source?.git?.ref);
      expect(manifest.status?.build?.digest, `${app} runs an image by digest`).toMatch(/^sha256:[0-9a-f]{64}$/);

      const served = new RegExp(`^Served ${(manifest.status?.build?.commit ?? "").slice(0, 7)}`);
      await expect(viewer.page.getByText(served).first(), `${app}'s card says the commit it serves`).toBeVisible();
    }
  } finally {
    await viewer.context.close();
  }
});

// AP-28, AP-71: the public sample opens with no login and its buses move between two polls.
test("anyone opens hsl-transport without a login and the buses move", async ({ browser }) => {
  const anonymous = await browser.newContext();
  try {
    const page = await anonymous.newPage();
    await page.goto(`${APPS_URL}/apps/hsl-transport/`, { waitUntil: "load" });
    await expect(page.getByRole("heading", { level: 1, name: "HSL live" })).toBeVisible({ timeout: 60_000 });
    await expect(page.locator("#username"), "no login stands in front of a public app").toHaveCount(0);
    await expect(page.getByRole("status")).toHaveText(/^\d+ vehicles/, { timeout: 120_000 });

    type Vehicle = { id: string; coordinates: [number, number] };
    const read = async () => {
      const answer = await page.request.get(`${APPS_URL}/apps/hsl-transport/api/vehicles`);
      expect(answer.status(), await answer.text()).toBe(200);
      return new Map(((await answer.json()) as Vehicle[]).map((vehicle) => [vehicle.id, vehicle.coordinates.join(",")]));
    };
    const before = await read();
    expect(before.size, "the space holds buses").toBeGreaterThan(0);
    await expect
      .poll(
        async () => {
          const after = await read();
          return [...after].some(([id, at]) => before.has(id) && before.get(id) !== at);
        },
        { timeout: 180_000, intervals: [15_000] },
      )
      .toBe(true);
  } finally {
    await anonymous.close();
  }
});

// AP-40, AP-109, UI-44: the viewer reads the stations, holds the viewer role, sees Edit disabled
// with the reason, and a write sent past the page is refused by the gateway.
test("a viewer reads the air-quality stations, gets no form and is refused a write", async ({ browser }) => {
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/apps?lang=en`);
  try {
    const page = viewer.page;
    await goSignedIn(page, VIEWER, AIR, (opened) => opened.getByRole("heading", { level: 1, name: "Air quality" }));
    await expect(page.getByRole("listitem").first()).toBeVisible({ timeout: 120_000 });
    const me = (await (await page.request.get(`${AIR}api/me`)).json()) as { roles: string[] };
    expect(me.roles).toEqual(["viewer"]);
    const edit = page.getByRole("button", { name: /^Edit / }).first();
    await expect(edit).toBeDisabled();
    await expect(edit).toHaveAccessibleDescription("Only a steward adds, corrects or removes station records.");
    await expect(page.getByRole("form")).toHaveCount(0);

    const [first] = await stations(page);
    expect(first?.id, "the space holds a station to try").toMatch(/^urn:ngsi-ld:AirQualityObserved:/);
    const write = await page.request.patch(`${AIR}api/stations/${encodeURIComponent(first.id)}`, {
      data: { stewardNote: "written by a viewer" },
    });
    expect(write.status(), "the steward's grant is the steward's alone (AP-96)").toBe(403);
    expect(write.headers()["content-type"]).toContain("application/problem+json");
  } finally {
    await viewer.context.close();
  }
});

// AP-09, AP-62, AP-96: the steward adds a station, corrects its note and removes it; the viewer
// sees the added station, and a pipeline's station cannot be removed.
test("a steward adds, corrects and removes a station, and the viewer sees it", async ({ browser }) => {
  const steward = await signIn(browser, STEWARD, `/projects/${PROJECT}/apps?lang=en`);
  const viewer = await signIn(browser, VIEWER, `/projects/${PROJECT}/apps?lang=en`);
  const page = steward.page;
  try {
    await goSignedIn(page, STEWARD, AIR, (opened) => opened.getByRole("heading", { level: 1, name: "Air quality" }));
    const add = page.getByRole("form", { name: "New station" });
    await expect(add).toBeVisible({ timeout: 60_000 });
    await add.getByLabel("Station id").fill(LOCAL_ID);
    await add.getByLabel("Name in Finnish").fill(NAME);
    await add.getByLabel("Longitude").fill("24.9521");
    await add.getByLabel("Latitude").fill("60.1699");
    await add.getByRole("button", { name: "Add station" }).click();
    await expect(page.getByRole("heading", { name: NAME })).toBeVisible({ timeout: 60_000 });

    await goSignedIn(viewer.page, VIEWER, AIR, (opened) => opened.getByRole("heading", { level: 1, name: "Air quality" }));
    await expect(viewer.page.getByRole("heading", { name: NAME }), "the viewer sees the steward's station").toBeVisible({
      timeout: 60_000,
    });

    await page.getByRole("button", { name: `Edit ${NAME}` }).click();
    const edit = page.getByRole("form", { name: `Edit ${NAME}` });
    await edit.getByLabel("Steward note").fill("Checked on site by the T-2617 journey.");
    await edit.getByRole("button", { name: "Save changes" }).click();
    await expect(page.getByText("Note: Checked on site by the T-2617 journey.")).toBeVisible({ timeout: 60_000 });

    // FMI's stations carry `source`: the steward may correct them, not remove them.
    const pipeline = (await stations(page)).find((station) => !station.own);
    expect(pipeline, "the pipeline wrote FMI's stations").toBeDefined();
    await expect(page.getByRole("button", { name: `Remove ${pipeline?.name ?? ""}` })).toBeDisabled();

    await page.getByRole("button", { name: `Remove ${NAME}` }).click();
    await page.getByRole("button", { name: `Confirm removal of ${NAME}` }).click();
    await expect(page.getByRole("heading", { name: NAME })).toHaveCount(0, { timeout: 60_000 });
  } finally {
    // A run that died between the add and the remove leaves the one fixed record: remove it.
    const left = (await stations(page).catch(() => [])).find((station) => station.id.endsWith(`:${LOCAL_ID}`));
    if (left) {
      await page.request.delete(`${AIR}api/stations/${encodeURIComponent(left.id)}`);
    }
    await viewer.context.close();
    await steward.context.close();
  }
});
