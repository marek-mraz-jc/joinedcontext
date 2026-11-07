/**
 * T-3258: a pipeline recipe asks only for its source and where it writes, and fills the form the
 * workbench samples, maps and tests before anything is proposed.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { RecipeGallery, sourceLabel } from "../src/pages/pipelines/RecipeGallery";
import { RECIPES, fromRecipe, nameOf, problemOf } from "../src/pages/pipelines/recipes";

const r = en.pipelines.recipe;
const recipe = (id: string) => RECIPES.find((one) => one.id === id)!;
const manifest = (kind: string, name: string, spec: Record<string, unknown> = {}): Manifest => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: "helsinki" },
  spec,
});
const ENDPOINTS = [
  manifest("Endpoint", "bikes-read", { contextSpaceRef: "bikes" }),
  manifest("Endpoint", "kpi-write", { contextSpaceRef: "helsinki-kpi" }),
];
const TARGET = "urn:ngsi-ld:Endpoint:hel.fi:bikes:bikes-write";
const context = { project: "helsinki", endpoints: ENDPOINTS, orgDomain: "hel.fi" };

describe("pipeline recipes", () => {
  it("fills a load recipe with its source, target and timing, and leaves the mapping to the workbench", () => {
    expect(fromRecipe(recipe("csv"), { source: "stations-csv", target: TARGET }, context)).toEqual({
      name: "stations-csv-to-bikes-write",
      class: "scheduled",
      schedule: "0 3 * * *",
      source: { dataSourceRef: "stations-csv" },
      output: { mode: "upsert" },
      targetEndpoint: TARGET,
    });
    expect(fromRecipe(recipe("ckan"), { source: "open-data", target: TARGET }, context)).toMatchObject({ schedule: "0 4 * * *" });
    expect(fromRecipe(recipe("api"), { source: "gbfs", target: TARGET }, context)).toMatchObject({ class: "auto", period: "15m" });
  });

  it("fills the KPI recipe with its aggregation and the project's KPI endpoint", () => {
    const form = fromRecipe(recipe("kpi"), { source: "bikes-read", type: "BikeHireDockingStation", attribute: "availableBikeNumber" }, context);
    expect(form).toMatchObject({
      name: "bikes-read-availablebikenumber-avg",
      period: "15m",
      source: { endpointRef: "bikes-read", query: { type: "BikeHireDockingStation", attrs: ["availableBikeNumber"] } },
      output: { type: "KeyPerformanceIndicator", mode: "upsert" },
    });
    expect(form.compute?.bloblang).toContain("availableBikeNumber");
    expect(form.targetEndpoint).toMatch(/kpi-write$/);
  });

  it("refuses what a recipe still needs, and names a pipeline as a DNS label", () => {
    expect(problemOf(recipe("csv"), { source: "" })).toBe("source");
    expect(problemOf(recipe("csv"), { source: "a" })).toBe("target");
    expect(problemOf(recipe("kpi"), { source: "a", type: "" })).toBe("type");
    expect(problemOf(recipe("kpi"), { source: "a", type: "T", attribute: "pm 10" })).toBe("attribute");
    expect(problemOf(recipe("kpi"), { source: "a", type: "T", attribute: "pm10" })).toBeUndefined();
    expect(nameOf("Kvalita Ovzdušia", "to", "x".repeat(80))).toMatch(/^kvalita-ovzdusia-to-x+$/);
    expect(nameOf("")).toBe("pipeline");
  });
});

describe("the recipe gallery", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("asks a load recipe for its source and target, says what is missing, and hands over the form", async () => {
    const user = userEvent.setup();
    const onUse = vi.fn();
    render(
      <I18nextProvider i18n={i18n}>
        <RecipeGallery
          project="helsinki"
          orgDomain="hel.fi"
          dataSources={[manifest("DataSource", "gbfs")]}
          endpoints={ENDPOINTS}
          targets={[{ name: "bikes-write", urn: TARGET }, { name: "unpublished" }]}
          onUse={onUse}
        />
      </I18nextProvider>,
    );
    await user.click(screen.getByRole("button", { name: new RegExp(r.api.title) }));
    await user.click(screen.getByRole("button", { name: r.use }));
    expect(screen.getByText(r.problem.source)).toBeInTheDocument();
    await user.selectOptions(screen.getByLabelText(new RegExp(r.api.source)), "gbfs");
    // An Endpoint with no URN cannot be written through and is not offered.
    expect(screen.queryByRole("option", { name: "unpublished" })).toBeNull();
    await user.selectOptions(screen.getByLabelText(new RegExp(r.target)), TARGET);
    await user.click(screen.getByRole("button", { name: r.use }));
    expect(onUse).toHaveBeenCalledWith(expect.objectContaining({ source: { dataSourceRef: "gbfs" }, period: "15m", targetEndpoint: TARGET }));
  });

  it("names each source by its title beside its name, and offers no App Endpoint (T-6572)", async () => {
    const user = userEvent.setup();
    const info: Manifest = {
      ...manifest("DataSource", "hsl-citybikes-gbfs-info"),
      metadata: { name: "hsl-citybikes-gbfs-info", namespace: "helsinki", title: { en: "City bike station information (GBFS)", fi: "Kaupunkipyöräasemien tiedot" } },
    };
    expect(sourceLabel(info, "en")).toBe("City bike station information (GBFS) (hsl-citybikes-gbfs-info)");
    expect(sourceLabel(manifest("DataSource", "gbfs"), "en")).toBe("gbfs");
    render(
      <QueryClientProvider client={new QueryClient()}>
      <I18nextProvider i18n={i18n}>
        <RecipeGallery
          project="helsinki"
          orgDomain="hel.fi"
          dataSources={[info]}
          endpoints={[...ENDPOINTS, manifest("Endpoint", "app-bikes-map", { contextSpaceRef: "bikes" })]}
          targets={[{ name: "bikes-write", urn: TARGET }]}
          onUse={vi.fn()}
        />
      </I18nextProvider>
      </QueryClientProvider>,
    );
    await user.click(screen.getByRole("button", { name: new RegExp(r.api.title) }));
    expect(screen.getByRole("option", { name: "City bike station information (GBFS) (hsl-citybikes-gbfs-info)" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: new RegExp(r.kpi.title) }));
    expect(screen.getByRole("option", { name: "bikes-read" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "app-bikes-map" })).toBeNull();
  });

  it("says where to start when the project has no data source", async () => {
    const user = userEvent.setup();
    render(
      <I18nextProvider i18n={i18n}>
        <RecipeGallery project="helsinki" dataSources={[]} endpoints={[]} targets={[]} onUse={() => undefined} />
      </I18nextProvider>,
    );
    await user.click(screen.getByRole("button", { name: new RegExp(r.csv.title) }));
    expect(screen.getByText(r.none.datasource)).toBeInTheDocument();
  });
});
