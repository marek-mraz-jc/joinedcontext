/**
 * T-7062 (G-18): a Barcelona tree register mapped in the mapper without typing Bloblang: a
 * Relationship to the District by its code, a bilingual name (ca + es), a point in ETRS89 / UTM
 * 31N and a status translated to the model's values. The step it writes reads back, and axe finds
 * nothing on the open editors.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { useState } from "react";
import axe from "axe-core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { OutputMapper } from "../src/pages/pipelines/OutputMapper";
import { LANGUAGE_TAGS, sampleValues } from "../src/pages/pipelines/MapperSources";
import type { Expiry, Lifetime } from "../src/pages/pipelines/lifetime";

const ATTRIBUTES = {
  model: "bb-air-quality",
  version: "1.0.0",
  type: "AirQualityObserved",
  description: "A street tree of the city register.",
  attributes: [
    { name: "name", kind: "LanguageProperty", required: false },
    { name: "refDistrict", kind: "Relationship", required: false, relationship: { target: "District", many: false } },
    { name: "location", kind: "GeoProperty", required: false },
    { name: "status", kind: "Property", valueType: "string", required: false, values: ["working", "closed"] },
  ],
};

const RECORDS = [
  { ID: "T1", NOM_CA: "Plàtan", NOM_ES: "Plátano", CODI_DISTRICTE: "01", X: "430887.56", Y: "4581837.85", ESTAT: " Bo " },
  { ID: "T2", NOM_CA: "Pi", NOM_ES: "Pino", CODI_DISTRICTE: "02", X: "430900.00", Y: "4581800.00", ESTAT: "Dolent" },
];

beforeEach(() => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(typeof input === "string" || input instanceof URL ? String(input) : input.url, window.location.origin);
      if (url.pathname === "/api/v1/organization/datamodels") {
        return Response.json({
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "List",
          items: [{ name: "bb-air-quality", level: "project", project: "bb", space: "ovzdusie", version: "1.0.0", lifecycle: "published", classes: ["AirQualityObserved"] }],
          smartDataModels: [],
        });
      }
      if (url.pathname === "/api/v1/projects/bb/datamodels/bb-air-quality/source") {
        return new Response("classes: {}\n", { headers: { "Content-Type": "text/yaml" } });
      }
      if (url.pathname === "/api/v1/projects/bb/spaces/ovzdusie/types/AirQualityObserved/attributes") {
        return Response.json(ATTRIBUTES);
      }
      return new Response("not found", { status: 404 });
    }),
  );
});

afterEach(() => vi.unstubAllGlobals());

let written: string[] = [];
let swept: (Expiry | undefined)[] = [];
let lifetimes: (Lifetime[] | undefined)[] = [];

function Host({ initial }: { initial: string }) {
  const [bloblang, setBloblang] = useState(initial);
  const [expiry, setExpiry] = useState<Expiry | undefined>(undefined);
  const [lifetime, setLifetime] = useState<Lifetime[] | undefined>(undefined);
  return (
    <OutputMapper
      project="bb"
      space="ovzdusie"
      orgDomain="banskabystrica.sk"
      records={RECORDS}
      bloblang={bloblang}
      onBloblang={(next) => {
        written.push(next);
        setBloblang(next);
      }}
      expiry={expiry}
      lifetime={lifetime}
      onEnds={({ expiry: next, lifetime: ends, bloblang: step }) => {
        swept.push(next);
        setExpiry(next);
        lifetimes.push(ends);
        setLifetime(ends);
        if (step !== undefined) {
          written.push(step);
          setBloblang(step);
        }
      }}
    />
  );
}

/** The mapper on a page of its own, beside the route its link to the data model opens. */
function show(initial = "") {
  written = [];
  swept = [];
  lifetimes = [];
  const root = createRootRoute({ component: Outlet });
  const page = createRoute({ getParentRoute: () => root, path: "/", component: () => <Host initial={initial} /> });
  const elsewhere = createRoute({ getParentRoute: () => root, path: "$", component: () => <p>another page</p> });
  const router = createRouter({ routeTree: root.addChildren([page, elsewhere]) });
  window.history.pushState({}, "", "/");
  return render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </I18nextProvider>,
  );
}

async function pickType(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("combobox", { name: /Entity type/ }));
  await user.click(await screen.findByRole("option", { name: /AirQualityObserved/ }));
}

describe("the G-18 mapper sources, with no Bloblang typed", () => {
  it("maps a Relationship, a bilingual name, a UTM point and a translated status", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    const { container } = show();
    await pickType(user);
    const ref = await screen.findByTestId("mapper-attribute-refDistrict");
    await user.selectOptions(within(ref).getByRole("combobox", { name: /Source of refDistrict/ }), "ref");
    await user.selectOptions(within(ref).getByRole("combobox", { name: /Key field of the District/ }), "CODI_DISTRICTE");

    const name = screen.getByTestId("mapper-attribute-name");
    await user.selectOptions(within(name).getByRole("combobox", { name: /Source of name/ }), "languages");
    await user.selectOptions(within(name).getByRole("combobox", { name: /^Language/ }), "ca");
    await user.selectOptions(within(name).getByRole("combobox", { name: /Field for ca/ }), "NOM_CA");
    await user.click(within(name).getByRole("button", { name: "Add a language" }));
    await user.selectOptions(within(name).getAllByRole("combobox", { name: /^Language/ })[1], "es");
    await user.selectOptions(within(name).getByRole("combobox", { name: /Field for es/ }), "NOM_ES");

    const location = screen.getByTestId("mapper-attribute-location");
    await user.selectOptions(within(location).getByRole("combobox", { name: /Source of location/ }), "point");
    await user.selectOptions(within(location).getByRole("combobox", { name: /Longitude field/ }), "X");
    await user.selectOptions(within(location).getByRole("combobox", { name: /Latitude field/ }), "Y");
    await user.selectOptions(within(location).getByRole("combobox", { name: /Coordinate system/ }), "EPSG:25831");

    const status = screen.getByTestId("mapper-attribute-status");
    await user.selectOptions(within(status).getByRole("combobox", { name: /Source of status/ }), "field:ESTAT");
    await user.selectOptions(within(status).getByRole("combobox", { name: /Change the value/ }), "trim");
    await user.selectOptions(within(status).getByRole("combobox", { name: /Change the value/ }), "lower");
    await user.selectOptions(within(status).getByRole("combobox", { name: /Change the value/ }), "lookup");
    await user.selectOptions(within(status).getByRole("combobox", { name: /“bo” becomes/ }), "working");
    await user.selectOptions(within(status).getByRole("combobox", { name: /“dolent” becomes/ }), "closed");

    const step = written.at(-1) ?? "";
    expect(step).toContain('"urn:ngsi-ld:%v:%v:%v:%v".format("District", $owner, "ovzdusie", this.CODI_DISTRICTE');
    expect(step).toContain('"ca": if this.NOM_CA != null');
    expect(step).toContain('"es": if this.NOM_ES != null');
    expect(step).toContain('.geo_transform("EPSG:25831", "EPSG:4326")');
    expect(step).toContain('match this.ESTAT.string().trim().string().lowercase().string() { "bo" => "working", "dolent" => "closed", _ => this }');
    // The step is still the mapper's: it reads back, so the form keeps showing it.
    expect(screen.queryByText(/written by hand/)).toBeNull();
    const shown = screen.getByTestId("mapper-preview").textContent ?? "";
    expect(shown).toContain("urn:ngsi-ld:District:banskabystrica.sk:ovzdusie:01");
    expect(shown).toContain('"es": "Plátano"');
    expect(shown).toContain('"value": "closed"');
    expect(within(status).queryByText(/takes one of/)).toBeNull();

    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`)).toEqual([]);
  });

  it("lists a lookup's sample values once each and offers the cities' languages", () => {
    expect(sampleValues([...RECORDS, { ESTAT: " Bo " }, { ESTAT: "" }], "ESTAT")).toEqual([" Bo ", "Dolent"]);
    expect(LANGUAGE_TAGS).toEqual(expect.arrayContaining(["ca", "es", "sk", "fi"]));
  });

  it("adds a default after a transform and keeps it last", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    show();
    await pickType(user);
    const status = await screen.findByTestId("mapper-attribute-status");
    await user.selectOptions(within(status).getByRole("combobox", { name: /Source of status/ }), "field:ESTAT");
    await user.selectOptions(within(status).getByRole("combobox", { name: /Change the value/ }), "default");
    await user.selectOptions(within(status).getByRole("combobox", { name: /Change the value/ }), "trim");
    const steps = within(within(status).getByRole("list", { name: /Transforms of status/ })).getAllByRole("listitem");
    expect(steps.map((item) => item.textContent?.split("Remove")[0])).toEqual(["Trim spaces", "Value when empty"]);
    expect(written.at(-1)).toContain('else { "working" }');
  });
});
