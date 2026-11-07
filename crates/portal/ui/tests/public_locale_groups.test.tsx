/**
 * T-5165: a visitor's catalogue and dataset pages load only the public groups of a locale
 * (src/locales/public-groups.json, vite.config.ts publicLocales). A word outside them makes the
 * page download the whole locale (about 126 KB gzipped) and draw again, the cost the public entry
 * exists to save: measured on the dataset page once its quality score (group `quality`) shipped.
 */
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import groups from "../src/locales/public-groups.json";
import { PublicApp } from "../src/PublicApp";
import { jsonResponse } from "./pageHarness";

const DATASET = {
  name: "bbsk-kpi",
  title: "Ukazovatele kraja",
  notes: "Ukazovatele Banskobystrického samosprávneho kraja.",
  keywords: ["kraj"],
  publisher: { name: "bbsk", title: "Banskobystrický samosprávny kraj" },
  licence: { id: "cc-by", title: "Creative Commons Attribution" },
  frequency: "http://publications.europa.eu/resource/authority/frequency/MONTHLY",
  formats: ["CSV", "NGSI-LD"],
  themes: [{ code: "ECON", label: "Economy and finance" }],
  spatial: ["SK032"],
  catalogueUrl: "https://opendata.dev.joinedcontext.com/dataset/bbsk-kpi",
  resources: [{ name: "CSV", format: "CSV", url: "https://api.dev.joinedcontext.com/api/endpoint/k7m2/file.csv" }],
  endpoint: { url: "https://api.dev.joinedcontext.com/api/endpoint/k7m2/", representations: ["ngsi-ld", "csv", "ical"] },
  quality: { score: 95, measuredAt: "2026-10-22T08:04:00Z", parts: [] },
};

const PAGE = {
  total: 1,
  page: 1,
  pageSize: 20,
  datasets: [{ ...DATASET, themes: ["ECON"] }],
  facets: { publisher: [], theme: [], format: [], licence: [], spatial: [], year: [] },
  unavailable: [],
};

const missing: string[] = [];
const onMissing = (_languages: readonly string[], _namespace: string, key: string) => void missing.push(key);

beforeAll(() => {
  // What the built entry holds for English: its public groups and nothing else.
  i18n.removeResourceBundle("en", "translation");
  i18n.addResourceBundle("en", "translation", Object.fromEntries(groups.map((group) => [group, en[group as keyof typeof en]])));
  i18n.on("missingKey", onMissing);
});

afterAll(() => {
  i18n.off("missingKey", onMissing);
  i18n.addResourceBundle("en", "translation", en, true, true);
});

afterEach(() => {
  vi.restoreAllMocks();
  window.history.pushState({}, "", "/");
});

function visit(path: string, dataset: object = DATASET) {
  missing.length = 0;
  window.history.pushState({}, "", path);
  vi.spyOn(globalThis, "fetch").mockImplementation((input: RequestInfo | URL) => {
    const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
    if (url.pathname.endsWith("/auth/me")) return Promise.resolve(jsonResponse({ title: "Unauthorized" }, 401));
    if (url.pathname === "/api/v1/catalogue") return Promise.resolve(jsonResponse(PAGE));
    if (url.pathname === "/api/v1/catalogue/datasets/bbsk-kpi") return Promise.resolve(jsonResponse(dataset));
    return Promise.resolve(jsonResponse({ title: "Not Found" }, 404));
  });
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <PublicApp />
    </QueryClientProvider>,
  );
}

describe("the public pages' words", () => {
  it("draws the catalogue from the public groups alone", async () => {
    visit("/catalogue");
    expect(await screen.findByRole("link", { name: "Ukazovatele kraja" })).toBeInTheDocument();
    expect(missing).toEqual([]);
  });

  it("draws a dataset with its quality score and subscribe menu from the public groups alone", async () => {
    visit("/catalogue/bbsk-kpi");
    expect(await screen.findByRole("heading", { level: 1, name: "Ukazovatele kraja" })).toBeInTheDocument();
    expect(await screen.findByTestId("dataset-quality")).toBeInTheDocument();
    // The menu loads beside the page (lazy): a disabled stand-in of the same size, then the menu.
    await waitFor(() => expect(screen.getByRole("button", { name: "Subscribe" })).toBeEnabled());
    expect(missing).toEqual([]);
  });

  // T-5165: the menu loads lazily behind a stand-in button; a dataset with nothing to subscribe to
  // gets neither, or the stand-in would vanish and move the page under it (layout shift).
  it("draws no subscribe button, not even its stand-in, for a dataset with nothing to subscribe to", async () => {
    visit("/catalogue/bbsk-kpi", { ...DATASET, endpoint: { ...DATASET.endpoint, representations: ["ngsi-ld", "csv"], feeds: [], calendars: [] } });
    expect(await screen.findByRole("heading", { level: 1, name: "Ukazovatele kraja" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Subscribe" })).toBeNull();
  });
});
