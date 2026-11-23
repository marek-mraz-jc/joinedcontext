import { expect, test } from "@playwright/test";
import type { Page, Route } from "@playwright/test";
import { axeViolations } from "./axe";
import { manifestReadsAsRest } from "./manifestReads";
import { writePhoneReport } from "./phoneCheck.cts";

// The pages read their manifests through the OData service; this file stubs the REST reads (T-7073).
test.beforeEach(({ page }) => manifestReadsAsRest(page));

// T-6875 (ODP-15): the Portals area on stubbed routes. Every state of the list and the editor at
// 1440 and 400 px, and one create, edit and delete journey whose requests are each a checked
// proposal (a dry run, then the write) on the Portal kind's own routes.

const HOSTS = { items: [{ organization: "bbsk", hosts: ["doprava.bbsk.sk", "skoly.bbsk.sk"], opendata: "https://opendata.bbsk.sk", sites: [] as unknown[], bases: [] as string[] }] };

// T-7415: bbsk serving bbsk.smartcity.joinedcontext.com, with a verified domain and two own hosts.
const OWN_HOSTS = {
  items: [
    {
      ...HOSTS.items[0],
      bases: ["bbsk.smartcity.joinedcontext.com"],
      verifiedDomain: "bbsk.sk",
      sites: [
        { host: "opendata2.bbsk.smartcity.joinedcontext.com", state: "dns-ok", detail: "the certificate waits until 2026-11-20 10:00 UTC (10 new certificates a day)", removable: true },
        { host: "data.bbsk.sk", state: "served", removable: true },
      ],
    },
  ],
};

function portal(path: string, title: Record<string, string>, extra: Record<string, unknown> = {}) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Portal",
    metadata: { name: `bbsk-${path}`, namespace: "org" },
    spec: { organizationRef: "bbsk", path, title, languages: Object.keys(title), ...extra },
  };
}

const FILLED = [
  portal("default", { sk: "Otvorené údaje BBSK", en: "BBSK open data" }, { sources: [{ name: "endpoints", type: "endpoints" }] }),
  portal("mobility", { sk: "Doprava", en: "Mobility" }, { host: "doprava.bbsk.sk", slug: "redirect", sources: [{ name: "kosice", type: "dcat-ap" }, { name: "nkod", type: "nkod-sparql" }] }),
  portal("skoly", { sk: "Školy", en: "Schools" }),
];

const CHANGE = {
  id: "c-1",
  title: "Propose Portal bbsk-mobility",
  lane: "Yellow",
  author: "jana",
  createdAt: "2026-11-14T10:00:00Z",
  status: { phase: "AwaitingApproval" },
  resources: [{ kind: "Portal", name: "bbsk-mobility", namespace: "org", operation: "create" }],
};

interface Stub {
  portals: unknown[];
  /** What a write of a portal answers; the default is a Change. */
  write?: (route: Route) => Promise<void>;
  /** The hosts read; a 403 for somebody who makes no portal. */
  hosts?: number;
  /** The hosts answer, when not HOSTS. */
  hostsBody?: unknown;
  /** What a write of a portal host answers; the default is the hosts answer. */
  hostWrite?: (route: Route) => Promise<void>;
  /** Takes the body of each proposed write, as its REST door reads it. */
  written?: unknown[];
  /** The datasets read of a portal (T-7520); 503 when false. */
  datasets?: unknown;
}

async function stub(page: Page, options: Stub): Promise<string[]> {
  const sent: string[] = [];
  await page.route(/\/api\//, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (path.endsWith("/auth/me")) return json({ subject: "s", username: "jana", name: "Jana Admin", email: "jana@bbsk.sk", roles: [] });
    if (path.endsWith("/permissions/me")) {
      return json({ project: "org", bootstrap: false, grants: [{ rule: { kinds: ["Portal", "Organization", "RoleBinding", "Group"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }] });
    }
    if (path === "/api/v1/projects") return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: "doprava" }] });
    if (path.startsWith("/api/v1/organization/portal-hosts") && request.method() !== "GET") {
      sent.push(`${request.method()} ${path}`);
      if (options.hostWrite) return options.hostWrite(route);
      return json((options.hostsBody as typeof HOSTS | undefined)?.items[0] ?? HOSTS.items[0], request.method() === "POST" ? 201 : 200);
    }
    if (path === "/api/v1/organization/portal-hosts") {
      if (options.hostsBody) return json(options.hostsBody);
      return options.hosts ? json({ type: "about:blank", title: "Forbidden", status: options.hosts, detail: "only an Organization's administrator" }, options.hosts) : json(HOSTS);
    }
    if (/^\/api\/v1\/projects\/org\/portals\/[^/]+\/datasets$/.test(path) && options.datasets !== undefined) {
      return options.datasets === false ? json({ type: "about:blank", title: "Service Unavailable", status: 503, detail: "no catalogue" }, 503) : json(options.datasets);
    }
    if (path.startsWith("/api/v1/projects/org/portals")) {
      if (request.method() !== "GET") {
        sent.push(`${request.method()} ${path}${url.search}`);
        if (request.method() !== "DELETE" && !url.searchParams.get("dryRun")) options.written?.push(request.postDataJSON());
        if (options.write) return options.write(route);
        return url.searchParams.get("dryRun") ? json({ valid: true, verdict: { ok: true, findings: [] } }) : json(CHANGE, 202);
      }
      const name = path.split("/")[6];
      if (!name) return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: options.portals });
      const found = (options.portals as { metadata: { name: string } }[]).find((p) => p.metadata.name === name);
      return found ? json(found) : json({ type: "about:blank", title: "Not Found", status: 404, detail: `Portal '${name}' not found` }, 404);
    }
    if (path.endsWith("/events")) return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
  });
  return sent;
}

async function shoot(page: Page, name: string, width: number, path: string): Promise<void> {
  // Full height in the viewport, so the sticky sidebar is not cut (knowledge/portal-ui.md).
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({ width, height: Math.max(height, 900) });
  await page.screenshot({ path });
}

for (const width of [1440, 400]) {
  test.describe(`at ${width} px`, () => {
    test.beforeEach(async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
    });

    test("the list: empty, filled, and refused", async ({ page }, info) => {
      await stub(page, { portals: [] });
      await page.goto("/portals?lang=en");
      await expect(page.getByRole("heading", { level: 1, name: "Open data portals" })).toBeVisible();
      await expect(page.getByText("No portal yet")).toBeVisible();
      await shoot(page, "empty", width, info.outputPath(`portals-list-empty-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);

      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stub(page, { portals: FILLED });
      await page.reload();
      await expect(page.getByRole("link", { name: "Mobility" })).toBeVisible();
      await expect(page.getByText("https://doprava.bbsk.sk/")).toBeVisible();
      await shoot(page, "filled", width, info.outputPath(`portals-list-filled-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
      expect(overflow).toBeLessThanOrEqual(0);

      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stub(page, { portals: [], hosts: 403 });
      await page.reload();
      await expect(page.getByText(/made by an Organization's administrators/)).toBeVisible();
      await shoot(page, "refusal", width, info.outputPath(`portals-list-refusal-${width}.png`));
    });

    test("the portal hosts: empty, filled, the server's refusal, remove, and nothing to add under", async ({ page }, info) => {
      const empty = { items: [{ ...OWN_HOSTS.items[0], sites: [] }] };
      await stub(page, { portals: FILLED, hostsBody: empty });
      await page.goto("/portals?lang=en");
      await expect(page.getByText(/No portal host of your own yet/)).toBeVisible();
      const panel = page.getByRole("heading", { level: 2, name: "Portal hosts" });
      await panel.scrollIntoViewIfNeeded();
      await shoot(page, "hosts-empty", width, info.outputPath(`portal-hosts-empty-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);

      await page.unrouteAll({ behavior: "ignoreErrors" });
      const sent = await stub(page, {
        portals: FILLED,
        hostsBody: OWN_HOSTS,
        hostWrite: (route) =>
          route.request().method() === "POST"
            ? route.fulfill({
                status: 422,
                contentType: "application/problem+json",
                body: JSON.stringify({ type: "about:blank", title: "Unprocessable", status: 422, detail: "x", errors: ["host: the label 'api' is reserved for the platform; choose another, e.g. opendata2"] }),
              })
            : route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(OWN_HOSTS.items[0]) }),
      });
      await page.reload();
      await expect(page.getByText("Certificate pending")).toBeVisible();
      await expect(page.getByText("Served", { exact: true })).toBeVisible();
      await shoot(page, "hosts-filled", width, info.outputPath(`portal-hosts-filled-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
      expect(overflow).toBeLessThanOrEqual(0);

      await page.getByRole("button", { name: "Add host" }).click();
      await expect(page.getByText(/Type the host's name first/)).toBeVisible();
      await page.locator("input[id='portal-host-label-bbsk']").fill("api");
      await page.getByRole("button", { name: "Add host" }).click();
      await expect(page.getByText(/the label 'api' is reserved/)).toBeVisible();
      expect(sent).toContain("POST /api/v1/organization/portal-hosts");
      await shoot(page, "hosts-error", width, info.outputPath(`portal-hosts-error-${width}.png`));

      await page.getByRole("button", { name: "Remove host data.bbsk.sk" }).click();
      const dialog = page.getByRole("alertdialog");
      await expect(dialog.getByText(/cannot be added again/)).toBeVisible();
      await shoot(page, "hosts-remove", width, info.outputPath(`portal-hosts-remove-${width}.png`));
      await dialog.getByRole("button", { name: "Remove host" }).click();
      await expect.poll(() => sent).toContain("DELETE /api/v1/organization/portal-hosts/data.bbsk.sk");

      await page.unrouteAll({ behavior: "ignoreErrors" });
      await stub(page, { portals: FILLED });
      await page.reload();
      await expect(page.getByText(/verified no domain yet/)).toBeVisible();
      await shoot(page, "hosts-none", width, info.outputPath(`portal-hosts-none-${width}.png`));
    });

    test("the editor: filled, the server's refusal on its field, and another Organization's portal", async ({ page }, info) => {
      await stub(page, {
        portals: FILLED,
        write: (route) =>
          route.fulfill({
            status: 422,
            contentType: "application/problem+json",
            body: JSON.stringify({ type: "about:blank", title: "Unprocessable", status: 422, detail: "portals.max (6) reached", errors: ["spec.host: skoly.bbsk.sk is already the host of portal bbsk-skoly2"] }),
          }),
      });
      await page.goto("/portals/bbsk-mobility?lang=en");
      await expect(page.getByLabel(/^Domain/)).toHaveValue("doprava.bbsk.sk");
      await shoot(page, "edit", width, info.outputPath(`portals-editor-filled-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);

      await page.getByLabel(/^Domain/).selectOption("skoly.bbsk.sk");
      await page.getByRole("button", { name: "Propose the changes" }).click();
      await expect(page.getByText("skoly.bbsk.sk is already the host of portal bbsk-skoly2")).toBeVisible();
      await shoot(page, "error", width, info.outputPath(`portals-editor-error-${width}.png`));

      await page.goto("/portals/praha-skoly?lang=en");
      await expect(page.getByText("No portal praha-skoly in your Organizations")).toBeVisible();
      await shoot(page, "refusal", width, info.outputPath(`portals-editor-refusal-${width}.png`));
    });

    test("the sources tab: an ArcGIS REST and an OGC API Records source with their hints, and a refused address", async ({ page }, info) => {
      await stub(page, { portals: FILLED });
      await page.goto("/portals/bbsk-mobility?lang=en");
      await page.getByRole("tab", { name: "Sources" }).click();
      await page.getByRole("button", { name: "Add a source" }).click();
      await page.getByLabel(/^Type/).selectOption("arcgis-rest");
      await page.getByLabel(/^Name/).fill("gis");
      await page.getByLabel(/^Address/).fill("https://sampleserver6.arcgisonline.com/arcgis/rest/services");
      await expect(page.getByText(/each service becomes one dataset/)).toBeVisible();
      await shoot(page, "arcgis-rest", width, info.outputPath(`portals-sources-arcgis-rest-${width}.png`));
      expect(await axeViolations(page)).toEqual([]);

      await page.getByLabel(/^Type/).selectOption("ogc-records");
      await page.getByLabel(/^Address/).fill("https://demo.pygeoapi.io/master/collections/dutch-metadata");
      await expect(page.getByText(/each record becomes one dataset/)).toBeVisible();
      await shoot(page, "ogc-records", width, info.outputPath(`portals-sources-ogc-records-${width}.png`));

      await page.getByLabel(/^Address/).fill("http://demo.pygeoapi.io/master/collections/dutch-metadata");
      await page.getByRole("button", { name: "Propose the source" }).click();
      await expect(page.getByText(/Write an https address/)).toBeVisible();
      await shoot(page, "error", width, info.outputPath(`portals-sources-error-${width}.png`));
    });
  });
}

test("create, edit and delete a portal: each write a checked proposal", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const written: { metadata: { name: string }; spec: Record<string, unknown> }[] = [];
  const sent = await stub(page, { portals: FILLED, written });
  await page.goto("/portals?lang=en");
  await page.getByRole("link", { name: "New portal" }).first().click();
  await page.getByLabel(/Name in the address/).fill("mobility2");
  await page.getByLabel(/Title in Slovak/).fill("Doprava 2");
  await page.getByRole("radio", { name: "A domain of its own" }).check();
  await page.getByLabel(/^Domain/).selectOption("skoly.bbsk.sk");
  await page.getByLabel("Also keep the usual slug address").check();
  await expect(page.getByText("The portal will answer at https://skoly.bbsk.sk/")).toBeVisible();
  await page.getByRole("button", { name: "Propose the portal" }).click();
  await expect.poll(() => written.length).toBe(1);
  const body = written[0];
  expect(body.metadata.name).toBe("bbsk-mobility2");
  expect(body.spec).toMatchObject({ organizationRef: "bbsk", path: "mobility2", host: "skoly.bbsk.sk", slug: "keep", title: { sk: "Doprava 2" } });
  await expect(page.getByText(/waiting|approv/i).first()).toBeVisible();

  await page.goto("/portals/bbsk-skoly?lang=en");
  await page.getByLabel(/Title in English/).fill("Schools and kindergartens");
  await page.getByRole("button", { name: "Propose the changes" }).click();
  await expect.poll(() => sent.filter((s) => s.startsWith("PUT /api/v1/projects/org/portals/bbsk-skoly")).length).toBe(2);

  await page.goto("/portals/bbsk-skoly?lang=en");
  await page.getByRole("button", { name: /Remove portal/ }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("textbox").fill("bbsk-skoly");
  await dialog.getByRole("button", { name: "Propose removal" }).click();
  await expect.poll(() => sent.some((s) => s.startsWith("DELETE /api/v1/projects/org/portals/bbsk-skoly"))).toBe(true);
  expect(sent.filter((s) => s.startsWith("POST")).map((s) => s.includes("dryRun"))).toEqual([true, false]);
});

// T-7520 and T-7521: the profiles an administrator chooses, and what keeps a dataset out of a feed.
const PROFILES = [
  { id: "dcat-ap-3.0.1", title: { en: "DCAT-AP 3.0.1 (European Union)" }, always: true },
  { id: "dcat-ap-fi-2.0", title: { en: "DCAT-AP-FI 2.0" }, country: "FI", always: false },
  { id: "geodcat-ap-3.1.0", title: { en: "GeoDCAT-AP 3.1.0" }, always: false },
  {
    id: "dcat-ap-sk-2.0",
    title: { en: "DCAT-AP-SK 2.0" },
    country: "SK",
    always: false,
    publisherPrefix: "https://data.gov.sk/id/legal-subject/",
    blocked: "dcat-ap-sk-2.0 needs the publisher's IRI: set spec.catalog.publisher.uri on Organization helsinki, e.g. https://data.gov.sk/id/legal-subject/…",
  },
];
const HELSINKI = portal("helsinki", { en: "Helsinki open data", fi: "Helsingin avoin data" });
const profileHosts = (profiles: unknown[]) => ({ items: [{ ...HOSTS.items[0], profiles }] });
const CATALOGUE = {
  total: 2,
  missingPublisher: ["dcat-ap-fi-2.0"],
  profiles: ["dcat-ap-fi-2.0", "geodcat-ap-3.1.0"],
  items: [
    { name: "bike-stations", title: "City bike stations", source: "endpoints", hidden: false, missing: [] },
    {
      name: "tree-register",
      title: "Register of street trees",
      source: "endpoints",
      hidden: false,
      missing: [{ profile: "geodcat-ap-3.1.0", fields: ["applicable_legislation"] }],
    },
  ],
};

for (const width of [1440, 400]) {
  test.describe(`profiles at ${width} px`, () => {
    test.use({ viewport: { width, height: width === 400 ? 2300 : 900 }, hasTouch: width === 400, isMobile: width === 400 });

    test("the choice with none, some, a refusal and no answer, and the datasets out of a feed", async ({ page }, info) => {
      const state = async (name: string) => {
        const dir = process.env.JC_EVIDENCE_DIR;
        const shot = dir ? `${dir}/${name}-${width}.png` : info.outputPath(`${name}-${width}.png`);
        // A phone scales a taller viewport down, so it takes the whole page instead.
        // A resize or a full page shot drops the touch emulation: the phone is tall from the start.
        if (width === 400) await page.screenshot({ path: shot });
        else await shoot(page, name, width, shot);
        if (width === 400) expect(await writePhoneReport(page, shot)).toEqual([]);
        expect(await axeViolations(page)).toEqual([]);
        if (width !== 400) await page.setViewportSize({ width, height: 900 });
      };
      await page.emulateMedia({ reducedMotion: "reduce" });
      const field = page.getByRole("group", { name: "Catalogue profiles" });
      const open = async (options: Partial<Stub>) => {
        await page.unrouteAll({ behavior: "ignoreErrors" });
        const sent = await stub(page, { portals: [HELSINKI], hostsBody: profileHosts(PROFILES), ...options });
        await page.goto("/portals/bbsk-helsinki?lang=en");
        await expect(field).toBeVisible();
        return sent;
      };

      await open({ hostsBody: profileHosts(PROFILES.slice(0, 1)), datasets: { total: 0, missingPublisher: [], profiles: [], items: [] } });
      await expect(field.getByText(/No other profile is installed/)).toBeVisible();
      await state("profiles-empty");

      await open({ datasets: false });
      await expect(field.getByText(/could not be read/)).toBeVisible();
      await expect(field.getByRole("button", { name: "Read again" })).toBeVisible();
      await state("profiles-error");

      const asked: { spec: { profiles?: string[] } }[] = [];
      const refuse = (route: Route) => {
        asked.push(route.request().postDataJSON() as (typeof asked)[number]);
        return route.fulfill({
          status: 422,
          contentType: "application/problem+json",
          body: JSON.stringify({ type: "about:blank", title: "Unprocessable", status: 422, detail: "refused", errors: ["spec.profiles[0]: dcat-ap-fi-2.0 needs the publisher's IRI: set spec.catalog.publisher.uri on Organization helsinki"] }),
        });
      };
      const sent = await open({ datasets: { total: 0, missingPublisher: [], profiles: [], items: [] }, write: refuse });
      const fi = field.getByRole("checkbox", { name: "DCAT-AP-FI 2.0 (FI)" });
      await expect(fi).toBeEnabled();
      await expect(field.getByRole("checkbox", { name: "DCAT-AP-SK 2.0 (SK)" })).toBeDisabled();
      await state("profiles-choice");
      await fi.check();
      await page.getByRole("button", { name: "Propose the changes" }).click();
      await expect(field.getByRole("alert")).toContainText("was not saved");
      expect(sent.length).toBeGreaterThan(0);
      expect(asked[0].spec.profiles).toEqual(["dcat-ap-fi-2.0"]);
      await state("profiles-refusal");

      // Chosen while the Organization still had its publisher.
      const lost = PROFILES.map((p) => (p.id === "dcat-ap-fi-2.0" ? { ...p, blocked: "dcat-ap-fi-2.0 needs the publisher's IRI" } : p));
      await open({ datasets: CATALOGUE, hostsBody: profileHosts(lost) });
      await expect(fi).toBeChecked();
      await expect(field.getByText(/its feed is paused/)).toBeVisible();
      await state("profiles-filled");
      await page.getByRole("tab", { name: "Datasets" }).click();
      await expect(page.getByText("Missing publisher")).toBeVisible();
      await expect(page.getByText(/applicable_legislation/).filter({ visible: true }).first()).toBeVisible();
      await state("datasets-gaps");
    });
  });
}
