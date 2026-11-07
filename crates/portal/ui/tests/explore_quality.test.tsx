/**
 * T-3252 (DM-74): the Quality tab of one entity type in Explore reads the daily data-quality run
 * of the space: how complete each attribute is, the newest change, numeric ranges and the values
 * far outside the rest, and the pipelines writing the type. Ranges and ids are only there for
 * whoever reads the entities: the API leaves them out, and the tab then shows a dash.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { InRouter } from "./pageHarness";
import { queryKeys } from "../src/api/client";
import en from "../src/locales/en.json";
import { ExplorePage } from "../src/pages/explore/ExplorePage";
import { TypeQuality } from "../src/pages/explore/TypeQuality";

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const TYPE = "BikeHireDockingStation";
const QUALITY_KEY = ["projects", "helsinki", "spaces", "helsinki", "quality"];

const REPORT = {
  observedAt: "2026-10-07T02:00:00Z",
  checked: 10,
  invalid: 0,
  truncated: false,
  rules: [],
  freshness: [
    { pipeline: "citybikes", type: TYPE, newest: "2026-10-07T01:58:00Z", targetSeconds: 600, state: "fresh", paused: false },
    { pipeline: "streets", type: "Street", newest: null, targetSeconds: 600, state: "empty", paused: false },
  ],
  types: [
    {
      type: TYPE,
      count: 10,
      newest: "2026-10-07T01:58:00Z",
      attributes: [
        { name: "totalSlotNumber", present: 10, min: 2, max: 400, outliers: 1, outlierExamples: ["urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:031"] },
        { name: "name", present: 7, outliers: 0, outlierExamples: [] },
      ],
    },
  ],
};

function show(report: unknown, type = TYPE) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  client.setQueryData(QUALITY_KEY, report);
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <TypeQuality project="helsinki" space="helsinki" type={type} />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the quality of one entity type", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("shows each attribute's completeness, range and outliers, and the pipelines writing the type", async () => {
    show(REPORT);
    const table = await screen.findByRole("table", { name: `How complete each attribute of ${TYPE} is` });
    const rows = [
      within(table).getByRole("row", { name: /totalSlotNumber/ }),
      within(table).getByRole("row", { name: /totalSlotNumber/ }),
      within(table).getByRole("row", { name: /^name/ }),
    ];
    expect(rows[1]).toHaveTextContent("100%");
    expect(rows[1]).toHaveTextContent("2 – 400");
    expect(rows[1]).toHaveTextContent("1 value");
    expect(rows[1]).toHaveTextContent("urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:031");
    expect(rows[2]).toHaveTextContent("70%");
    expect(rows[2]).toHaveTextContent("7 of 10");
    expect(screen.getByText(/10 entities, the newest changed on/)).toBeInTheDocument();
    const pipelines = screen.getByRole("region", { name: en.explore.quality.pipelines });
    expect(within(pipelines).getByText("citybikes")).toBeInTheDocument();
    expect(within(pipelines).queryByText("streets")).toBeNull();
    expect(within(pipelines).getByRole("link", { name: "Open citybikes for its rejected records" })).toHaveAttribute(
      "href",
      "/projects/helsinki/pipelines",
    );
  });

  it("shows a dash where the report leaves a range out for a person who does not read the entities", async () => {
    const hidden = structuredClone(REPORT);
    delete (hidden.types[0].attributes[0] as { min?: number }).min;
    delete (hidden.types[0].attributes[0] as { max?: number }).max;
    hidden.types[0].attributes[0].outlierExamples = [];
    show(hidden);
    const table = await screen.findByRole("table");
    const slots = within(table).getByRole("row", { name: /totalSlotNumber/ });
    expect(slots).not.toHaveTextContent("400");
    expect(slots).toHaveTextContent("1 value");
  });

  it("says when the space was not checked yet, and when the run found no entity of the type", async () => {
    show({});
    expect(await screen.findByText(en.spaces.quality.notYet)).toBeInTheDocument();
  });

  it("says the run found none of a type it did not read", async () => {
    show(REPORT, "Street");
    expect(await screen.findByText("The last run found no Street entity in this space.")).toBeInTheDocument();
  });
});

describe("the explorer's Quality tab", () => {
  it("switches from the entities to the type's quality report", async () => {
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(new Response("[]", { status: 200, headers: { "Content-Type": "application/json" } }))));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
    client.setQueryData(queryKeys.list("helsinki", "endpoints"), list([
      {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "Endpoint",
        metadata: { name: "helsinki-bikes", namespace: "helsinki" },
        spec: { contextSpaceRef: "helsinki", slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf", audience: "public", enabledRepresentations: ["ngsi-ld"] },
      },
    ]));
    client.setQueryData(queryKeys.list("helsinki", "spaces"), list([
      { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "helsinki", namespace: "helsinki" }, spec: { dataModelRef: "helsinki-mobility" } },
    ]));
    client.setQueryData(queryKeys.list("helsinki", "datamodels"), list([
      {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "DataModel",
        metadata: { name: "helsinki-mobility", namespace: "helsinki" },
        spec: { classes: [TYPE], linkml: `id: https://hel.fi/m\nname: m\nclasses:\n  ${TYPE}:\n    slots: [name]\nslots:\n  name: {}\n` },
      },
    ]));
    client.setQueryData(QUALITY_KEY, REPORT);
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <ExplorePage project="helsinki" initialSpace="helsinki" initialEndpoint="helsinki-bikes" />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );
    // The grant arrives first: it moves the type into the group of the ones it does not grant.
    const types = await screen.findByLabelText(/Entity type/i);
    await waitFor(() => expect(types.querySelector("optgroup")).not.toBeNull());
    await userEvent.selectOptions(types, TYPE);
    const tabs = await screen.findByRole("tablist", { name: en.explore.views });
    await userEvent.click(within(tabs).getByRole("tab", { name: en.explore.quality.tab }));
    expect(await screen.findByTestId("type-quality")).toBeInTheDocument();
    expect(screen.getByRole("tabpanel")).toContainElement(screen.getByTestId("type-quality"));
    await userEvent.click(within(tabs).getByRole("tab", { name: en.explore.entities }));
    expect(screen.queryByTestId("type-quality")).toBeNull();
  });
});
