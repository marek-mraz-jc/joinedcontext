/**
 * Visual baselines of the pages a recording shows (T-0756, UI-01, UI-27, UI-28): the built
 * bundle, the API answered in the browser, one screenshot per page at the recording's size
 * (1920×1080 at zoom 1.5, laid out at 1280 CSS px) and at phone width. A baseline changes only
 * on purpose: `pnpm e2e -- visual --update-snapshots`, and the diff is reviewed like code.
 */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

const PROJECT = "helsinki";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const PROJECTS = { apiVersion: "joinedcontext.com/v1alpha1", kind: "ProjectList", items: [{ name: PROJECT }] };

// What the project holds of each quota, as `GET /api/v1/projects/{project}` answers it
// (PF-73, PF-75). Without it the quota card is simply not there, which is what a page looks
// like to a caller who may not read the project — not what these baselines are for.
const PROJECT_DETAIL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Project",
  metadata: { name: PROJECT },
  spec: {},
  status: {
    usage: {
      contextSpaces: { used: 2 },
      residentPipelines: { used: 1, limit: 4 },
      publicEndpoints: { used: 3, limit: 3 },
    },
  },
};

const SOURCE_URL = `https://git.example.sk/city/org/src/branch/main/projects/${PROJECT}/spaces/ovzdusie/space.yaml`;

function list(items: unknown[]) {
  return { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items };
}

const SPACES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: { name: "ovzdusie", namespace: PROJECT, title: { sk: "Ovzdušie", en: "Air quality" } },
    spec: { dataModelRef: "AirQualityObserved" },
    status: { phase: "Live", observedRevision: "9c1f0ab", sourceUrl: SOURCE_URL },
  },
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: { name: "doprava", namespace: PROJECT, title: { en: "Traffic" } },
    spec: {},
    status: { phase: "Pending" },
  },
]);

const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "public-air", namespace: PROJECT, title: { en: "Public air quality" } },
    spec: {
      contextSpaceRef: "ovzdusie",
      slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
      audience: "public",
      enabledRepresentations: ["ngsi-ld", "geojson", "csv"],
      rateLimits: { requestsPerMinute: 600 },
    },
    status: { phase: "Live", sourceUrl: SOURCE_URL },
  },
]);

const PIPELINES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Pipeline",
    metadata: { name: "aq-mqtt-ingest", namespace: PROJECT, title: { en: "Air quality MQTT" } },
    spec: { class: "resident", enabled: true, secretRefs: [{ name: "mqtt-credentials", key: "password", envVar: "MQTT_PASSWORD" }] },
    status: { phase: "Live", sourceUrl: SOURCE_URL },
  },
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Pipeline",
    metadata: { name: "hel-news", namespace: PROJECT, title: { en: "City news from the RSS feed" } },
    spec: { class: "batch", schedule: "*/15 * * * *", enabled: false },
    status: { phase: "Live" },
  },
]);

const APPS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "App",
    metadata: {
      name: "mapa-ovzdusia",
      namespace: PROJECT,
      title: { en: "Air quality map" },
      description: { en: "Stations coloured by PM10" },
    },
    spec: {
      kind: "ui",
      visibility: "project",
      lifecycle: "preview",
      embeddable: true,
      dataNeeds: [{ contextSpaceRef: { name: "ovzdusie" }, types: ["AirQualityObserved"], operations: ["queryEntity"] }],
    },
    status: { phase: "Live", observedRevision: "9f1c2ab", sourceUrl: SOURCE_URL },
  },
]);

const PROPOSAL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ChangeProposal",
  metadata: { name: "chg-1a2b3c4d", namespace: PROJECT },
  summary: { key: "change.summary.update", params: { kind: "Endpoint", name: "public-air", fields: 2 } },
  author: { name: "Marek Mráz", email: "marek@banskabystrica.sk" },
  createdAt: "2026-03-03T12:00:00Z",
  status: {
    lane: "yellow",
    phase: "PendingApproval",
    plan: { update: 1 },
    mergeRequest: "https://git.example.sk/city/org/pulls/7",
  },
  planFields: [
    { path: "spec.audience", from: "public", to: "internal" },
    { path: "spec.rateLimits.requestsPerMinute", from: 600, to: 300 },
  ],
};

const RUN = {
  id: "01J8ZQ4T7K9M2N3P4Q5R6S7T8V",
  project: PROJECT,
  appName: "ovzdusie-dnes",
  endpointName: "public-air",
  appClass: "fullstack",
  visibility: "project",
  prompt: "A map of the stations with today's PM10",
  status: "building",
  steps: 12,
  tokensUsed: 48_210,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-12T08:00:00Z",
};

const LISTS: Record<string, unknown> = {
  spaces: SPACES,
  endpoints: ENDPOINTS,
  pipelines: PIPELINES,
  apps: APPS,
  datamodels: list([]),
  datasources: list([]),
  changes: list([PROPOSAL]),
  "agent-runs": list([RUN]),
};

/**
 * What the twenty-page set (T-4888) answers besides: a person who may act in the project and
 * the organization, the organization itself and one catalogue dataset, so each page settles
 * on its content or its designed empty state instead of a skeleton.
 */
const MORE: Record<string, unknown> = {
  [`/api/v1/projects/${PROJECT}/permissions/me`]: { project: PROJECT, bootstrap: true, grants: [] },
  "/api/v1/projects/org/permissions/me": { project: "org", bootstrap: true, grants: [] },
  "/api/v1/projects/org/organizations": list([
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Organization",
      metadata: { name: "hel", namespace: "org" },
      spec: { domain: "hel.fi", locales: ["fi", "en"], defaultLocale: "fi" },
    },
  ]),
  [`/api/v1/projects/org/projects/${PROJECT}`]: {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Project",
    metadata: { name: PROJECT, namespace: "org", title: "Helsinki city data" },
    spec: { organizationRef: "hel" },
  },
  "/api/v1/organization/datamodels": list([]),
  // The Policies and limits section reads the catalog with the values in force (T-2715).
  "/api/v1/organization/limits": {
    entries: [
      { path: "spec.projects.quota.contextSpaces", section: "projects", security: false, default: null, min: 0, max: 20, value: 10, origin: "organization" },
      { path: "spec.limits.signIn.sessionIdleMinutes", section: "signIn", security: true, default: 60, min: 5, max: 120, value: null, origin: "default" },
    ],
    projects: [],
  },
  "/api/v1/projects/org/rolebindings": list([]),
  [`/api/v1/projects/${PROJECT}/knowledge/sources`]: list([]),
  // My AI connectors (T-5951): one read-only connector, so the page shows its card, not a refusal.
  "/api/v1/connectors": {
    items: [
      {
        id: "abcdefghijklmnopqrstuvwxyz",
        name: "Claude Desktop",
        owner: IDENTITY.subject,
        segments: ["data-read"],
        scope: [{ project: PROJECT, endpoint: "public-air", access: "read" }],
        writes: false,
        expiresAt: "2026-10-12T08:00:00Z",
        allowedClients: [],
        clientIsBoundary: false,
        state: "active",
        createdAt: "2026-09-11T08:00:00Z",
        lastUsedAt: "2026-09-12T07:55:00Z",
        url: "https://portal.dev.example/c/abcdefghijklmnopqrstuvwxyz",
      },
    ],
  },
  "/api/v1/catalogue": {
    total: 1,
    page: 1,
    pageSize: 20,
    datasets: [
      {
        name: "bbsk-kpi",
        title: "Ukazovatele kraja",
        notes: "Ukazovatele Banskobystrického samosprávneho kraja.",
        publisher: { name: "bbsk", title: "Banskobystrický samosprávny kraj" },
        licence: { id: "cc-by", title: "Creative Commons Attribution" },
        formats: ["CSV", "NGSI-LD"],
        themes: ["ECON"],
        modified: "2026-09-21T05:52:34Z",
      },
    ],
    facets: { publisher: [], theme: [], format: [], licence: [], spatial: [], year: [] },
    unavailable: [],
  },
};

// The project's Endpoint read on 28 of the last 30 days (T-5118), as the Endpoints page's Usage reads it.
const DAILY = Array.from({ length: 30 }, (_, n) => (n % 14 === 6 ? 0 : 60 + ((n * 37) % 90)));
const PROJECT_USAGE = {
  from: "2026-08-14",
  to: "2026-09-12",
  endpoints: [
    {
      name: "public-air",
      slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
      total: DAILY.reduce((a, b) => a + b, 0),
      totals: { "ngsi-ld": DAILY.reduce((a, b) => a + b, 0) - 56, mcp: 56 },
      daily: DAILY,
    },
  ],
};

/** The API from the browser: the lists above, the one change, the one run; the rest 404. */
async function stubApi(page: Page, more: Record<string, unknown> = {}): Promise<void> {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (route.request().method() !== "GET") {
      return json({ type: "about:blank", title: "Not in the baseline", status: 404 }, 404);
    }
    if (path.endsWith("/auth/me")) return json(IDENTITY);
    if (path in more) return json(more[path]);
    if (path === "/api/v1/projects") return json(PROJECTS);
    if (path === `/api/v1/projects/${PROJECT}`) return json(PROJECT_DETAIL);
    if (path === `/api/v1/projects/${PROJECT}/usage/endpoints`) return json(PROJECT_USAGE);
    // No other project shared an App: the home page's Shared with me shows its empty state.
    if (path === "/api/v1/shared-apps") return json([]);
    if (path.endsWith("/events")) {
      return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    }
    if (path.endsWith(`/agent-runs/${RUN.id}`)) return json(RUN);
    if (path.endsWith(`/changes/${PROPOSAL.metadata.name}`)) return json(PROPOSAL);
    const listed = path.match(new RegExp(`^/api/v1/projects/${PROJECT}/([a-z-]+)$`));
    if (listed && listed[1] in LISTS) return json(LISTS[listed[1]]);
    // Any other collection of the project is empty, so a page shows its designed empty state.
    if (listed) return json(list([]));
    if (path === "/api/v1/catalogue/readable") return json({ datasets: [] });
    if (path.startsWith("/api/endpoint/")) return json([]);
    return json({ type: "about:blank", title: "Not in the baseline", status: 404 }, 404);
  });
}

interface Screen {
  name: string;
  path: string;
  /** Anything to do once the page has settled, such as opening the assistant beside a run. */
  then?: (page: Page) => Promise<void>;
  /** Held at the design sizes (1440, 400) only, not at the recording's. */
  extra?: boolean;
}

const SCREENS: Screen[] = [
  { name: "spaces", path: `/projects/${PROJECT}/spaces` },
  { name: "endpoints", path: `/projects/${PROJECT}/endpoints` },
  { name: "pipelines", path: `/projects/${PROJECT}/pipelines` },
  { name: "space-complete", path: `/projects/${PROJECT}/spaces/complete` },
  { name: "approval", path: `/projects/${PROJECT}/approvals/${PROPOSAL.metadata.name}` },
  { name: "apps", path: `/projects/${PROJECT}/apps` },
  {
    name: "app-run-with-dock",
    path: `/projects/${PROJECT}/apps/${RUN.appName}`,
    then: async (page) => {
      const bubble = page.getByRole("button", { name: "Open the assistant" });
      if (await bubble.count()) {
        await bubble.first().click();
      }
    },
  },
  { name: "explore", path: `/projects/${PROJECT}/explore` },
  // T-4888: the rest of the twenty pages a person meets first, held at 1440 and 400 only.
  { name: "home", path: `/projects/${PROJECT}/home`, extra: true },
  { name: "approvals", path: `/projects/${PROJECT}/approvals`, extra: true },
  { name: "inbox", path: `/projects/${PROJECT}/inbox`, extra: true },
  { name: "activity", path: `/projects/${PROJECT}/activity`, extra: true },
  { name: "models", path: `/projects/${PROJECT}/models`, extra: true },
  { name: "access", path: `/projects/${PROJECT}/access`, extra: true },
  { name: "settings", path: `/projects/${PROJECT}/settings`, extra: true },
  { name: "knowledge", path: `/projects/${PROJECT}/knowledge`, extra: true },
  { name: "endpoint-new", path: `/projects/${PROJECT}/endpoints/new`, extra: true },
  { name: "catalogue", path: "/catalogue", extra: true },
  { name: "ai-connectors", path: "/ai-connectors", extra: true },
  { name: "organization", path: "/organization", extra: true },
];

const SIZES = [
  // The recording: 1920×1080 at zoom 1.5 lays the page out at 1280 CSS px.
  { name: "1920x1080-zoom1.5", viewport: { width: 1280, height: 720 }, deviceScaleFactor: 1.5 },
  { name: "400px", viewport: { width: 400, height: 860 }, deviceScaleFactor: 1 },
  // The design size of T-4888: every page of the design system pass at a common laptop width.
  { name: "1440", viewport: { width: 1440, height: 900 }, deviceScaleFactor: 1 },
];

for (const size of SIZES) {
  test.describe(size.name, () => {
    test.use({ viewport: size.viewport, deviceScaleFactor: size.deviceScaleFactor });

    for (const screen of SCREENS.filter((one) => !one.extra || !size.name.startsWith("1920"))) {
      test(`${screen.name} at ${size.name}`, async ({ page }) => {
        await stubApi(page, screen.extra ? MORE : {});
        // A frozen clock: "n s since the run started" and every other relative time hold still.
        await page.clock.setFixedTime(new Date("2026-09-12T08:10:00Z"));
        await page.goto(`${screen.path}?lang=en`, { waitUntil: "networkidle" });
        // Attached, not visible: at phone width the assistant, open beside a run, takes the screen.
        await expect(page.getByRole("main")).toBeAttached();
        await screen.then?.(page);
        await page.evaluate(() => document.fonts.ready);
        if (size.viewport.width <= 400) {
          // A phone scrolls down, never sideways (UI-27): a table wider than the screen scrolls
          // inside its own frame, and the columns it cannot show are marked `secondary`.
          const { overflow, widest } = await page.evaluate(() => {
            const doc = document.documentElement;
            const offenders: string[] = [];
            document.querySelectorAll<HTMLElement>("body *").forEach((el) => {
              const box = el.getBoundingClientRect();
              if (box.right > doc.clientWidth + 1 && el.offsetParent !== null) {
                offenders.push(`${el.tagName}.${String(el.className).slice(0, 70)} right=${Math.round(box.right)}`);
              }
            });
            return { overflow: doc.scrollWidth - doc.clientWidth, widest: offenders.slice(0, 6) };
          });
          expect(
            overflow,
            `${screen.name} makes the page scroll sideways: ${widest.join(" | ")}`,
          ).toBeLessThanOrEqual(1);
        }
        await expect(page).toHaveScreenshot(`${screen.name}-${size.name}.png`, {
          fullPage: true,
          animations: "disabled",
          caret: "hide",
          // Another machine's chromium rasterizes text a shade differently; a layout change is far above this.
          maxDiffPixelRatio: 0.03,
        });
      });
    }
  });
}
