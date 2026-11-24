/**
 * T-3224: the output node's mapper in the workbench's mapping step. Picking a type of the target
 * space lists its attributes from the space's model, matches the sample's fields by name and
 * writes the step at once; a required attribute left unmapped is said at that attribute; a field
 * chosen from the list rewrites the step; a step written by hand is never replaced unless the
 * person asks, and comes back on request.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { useState } from "react";
import axe from "axe-core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { OutputMapper } from "../src/pages/pipelines/OutputMapper";
import { generate, HEADER } from "../src/pages/pipelines/outputMapper";
import type { Attribute } from "../src/pages/pipelines/outputMapper";
import type { Expiry, Lifetime } from "../src/pages/pipelines/lifetime";

const ATTRIBUTES = {
  model: "bb-air-quality",
  version: "1.0.0",
  type: "AirQualityObserved",
  description: "One air-quality station and its latest hourly means.",
  attributes: [
    { name: "dateObserved", kind: "Property", valueType: "string", format: "date-time", required: true },
    { name: "pm10", kind: "Property", valueType: "number", required: false, unit: { code: "GQ", ucum: "ug/m3" } },
    { name: "status", kind: "Property", valueType: "string", required: false, values: ["working", "closed"] },
  ],
};

const RECORDS = [
  { id: "BB01", PM10: "18.4", status: "working", observed: "2026-10-07T08:00:00Z" },
  { id: "BB02", PM10: "21", status: "closed", observed: "2026-10-07T08:00:00Z" },
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

describe("the output mapper in the workbench", () => {
  it("lists the type's attributes, matches the sample's fields by name and writes the step at once", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    show();
    await pickType(user);
    await screen.findByTestId("mapper-attribute-dateObserved");
    expect(screen.getByText(ATTRIBUTES.description)).toBeInTheDocument();
    await waitFor(() => expect(written.length).toBeGreaterThan(0));
    const step = written.at(-1) ?? "";
    expect(step.startsWith(HEADER)).toBe(true);
    expect(step).toContain("this.PM10.number()");
    expect(step).toContain("root.status");
    // id is matched to the id field; dateObserved has no field of its name and is required.
    expect(screen.getByRole("combobox", { name: /Id from field/ })).toHaveValue("id");
    const date = screen.getByTestId("mapper-attribute-dateObserved");
    expect(within(date).getByText(/dateObserved is required/)).toBeInTheDocument();
    expect(screen.getByTestId("mapper-preview").textContent).toContain("urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:BB01");
  });

  it("maps a required attribute to a field the person picks and the problem goes", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    show();
    await pickType(user);
    const date = await screen.findByTestId("mapper-attribute-dateObserved");
    await user.selectOptions(within(date).getByRole("combobox", { name: /Source of dateObserved/ }), "field:observed");
    await waitFor(() => expect(within(date).queryByText(/is required/)).toBeNull());
    expect(written.at(-1)).toContain("this.observed.string()");
    expect(screen.getByTestId("mapper-preview").textContent).toContain("2026-10-07T08:00:00Z");
  });

  it("writes an expression, a fixed value, a joined text or nothing per attribute, each as chosen", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    show();
    await pickType(user);
    const date = await screen.findByTestId("mapper-attribute-dateObserved");
    const source = (row: HTMLElement, name: string) => within(row).getByRole("combobox", { name: new RegExp(`Source of ${name}`) });

    await user.selectOptions(source(date, "dateObserved"), "expression");
    const expression = within(date).getByRole("textbox", { name: /Bloblang expression for dateObserved/ });
    await user.clear(expression);
    await user.type(expression, "now()");
    await waitFor(() => expect(written.at(-1)).toContain("let a0 = (now())"));

    await user.selectOptions(source(date, "dateObserved"), "concat");
    await waitFor(() => expect(written.at(-1)).toContain(".join("));

    await user.selectOptions(source(date, "dateObserved"), "value");
    await user.type(within(date).getByRole("textbox", { name: /Fixed value of dateObserved/ }), "2026-01-01T00:00:00Z");
    await waitFor(() => expect(written.at(-1)).toContain('let a0 = "2026-01-01T00:00:00Z"'));

    const pm10 = screen.getByTestId("mapper-attribute-pm10");
    await user.selectOptions(source(pm10, "pm10"), "value");
    await user.type(within(pm10).getByRole("spinbutton", { name: /Fixed value of pm10/ }), "12.5");
    await waitFor(() => expect(written.at(-1)).toContain("let a1 = 12.5"));
    await user.clear(within(pm10).getByRole("spinbutton", { name: /Fixed value of pm10/ }));
    await waitFor(() => expect(written.at(-1)).toContain('let a1 = ""'));

    const status = screen.getByTestId("mapper-attribute-status");
    await user.selectOptions(source(status, "status"), "value");
    await user.selectOptions(within(status).getByRole("combobox", { name: /Fixed value of status/ }), "closed");
    await waitFor(() => expect(written.at(-1)).toContain('let a2 = "closed"'));

    await user.selectOptions(source(status, "status"), "");
    await waitFor(() => expect(written.at(-1)).not.toContain("root.status"));
  });

  it("says at each attribute what the sample cannot give it, and at the id when no field is chosen", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    show();
    await pickType(user);
    const pm10 = await screen.findByTestId("mapper-attribute-pm10");
    await user.selectOptions(within(pm10).getByRole("combobox", { name: /Source of pm10/ }), "field:status");
    expect(await within(pm10).findByText(/status holds “working”, which is not a number; pm10 takes a number/)).toBeInTheDocument();

    const status = screen.getByTestId("mapper-attribute-status");
    await user.selectOptions(within(status).getByRole("combobox", { name: /Source of status/ }), "field:PM10");
    expect(await within(status).findByText(/PM10 holds “18.4”; status takes one of working, closed/)).toBeInTheDocument();

    await user.selectOptions(within(pm10).getByRole("combobox", { name: /Observed at, for pm10/ }), "status");
    expect(await within(pm10).findByText(/status holds working, which is no time/)).toBeInTheDocument();
    expect(written.at(-1)).toContain('"observedAt": if this.status != null');
    await user.selectOptions(within(pm10).getByRole("combobox", { name: /Observed at, for pm10/ }), "");
    await waitFor(() => expect(written.at(-1)).not.toContain("observedAt"));

    const date = screen.getByTestId("mapper-attribute-dateObserved");
    await user.selectOptions(within(date).getByRole("combobox", { name: /Source of dateObserved/ }), "field:observed");
    await user.selectOptions(within(date).getByRole("combobox", { name: /Change the value/ }), "date");
    expect(await within(date).findByText(/observed holds “2026-10-07T08:00:00Z”, which the date format of dateObserved cannot read/)).toBeInTheDocument();

    await user.selectOptions(screen.getByRole("combobox", { name: /Id from field/ }), "");
    expect(await screen.findByText("Choose the field each entity's id comes from.")).toBeInTheDocument();
    expect(written.at(-1)).not.toContain("root.id");
  });

  it("joins fields, reads a date format, gives an empty field a value and says when a lookup has nothing to translate", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    show();
    await pickType(user);
    const date = await screen.findByTestId("mapper-attribute-dateObserved");
    await user.selectOptions(within(date).getByRole("combobox", { name: /Source of dateObserved/ }), "concat");
    await user.selectOptions(within(date).getByRole("combobox", { name: /^Field 1/ }), "status");
    await user.click(within(date).getByRole("button", { name: "Add a field" }));
    await user.click(within(date).getByRole("button", { name: "Remove the field PM10 from the join" }));
    await user.selectOptions(within(date).getByRole("combobox", { name: /Between the fields/ }), " - ");
    await waitFor(() => expect(written.at(-1)).toContain('[ if this.status != null && this.status != "" { this.status.string() } else { deleted() }, if this.id != null'));
    expect(written.at(-1)).toContain('.join(" - ")');

    await user.selectOptions(within(date).getByRole("combobox", { name: /Source of dateObserved/ }), "field:observed");
    await user.selectOptions(within(date).getByRole("combobox", { name: /Change the value/ }), "date");
    await user.selectOptions(within(date).getByRole("combobox", { name: /Date format of the field/ }), "%Y-%m-%d %H:%M:%S");
    await waitFor(() => expect(written.at(-1)).toContain('ts_strptime("%Y-%m-%d %H:%M:%S")'));
    await user.click(within(date).getByRole("button", { name: "Remove the step Date in a format" }));
    await waitFor(() => expect(written.at(-1)).not.toContain("ts_strptime"));

    const pm10 = screen.getByTestId("mapper-attribute-pm10");
    await user.selectOptions(within(pm10).getByRole("combobox", { name: /Change the value/ }), "default");
    await user.type(within(pm10).getByRole("spinbutton", { name: /Value when the field is empty/ }), "0");
    await waitFor(() => expect(written.at(-1)).toContain("} else { 0 }"));

    const status = screen.getByTestId("mapper-attribute-status");
    await user.selectOptions(within(status).getByRole("combobox", { name: /Change the value/ }), "default");
    await user.selectOptions(within(status).getByRole("combobox", { name: /Value when the field is empty/ }), "closed");
    await waitFor(() => expect(written.at(-1)).toContain('} else { "closed" }'));
    await user.selectOptions(within(status).getByRole("combobox", { name: /Change the value/ }), "lookup");
    expect(within(status).getByText(/Every value of the sample is already one of the model's/)).toBeInTheDocument();
  });

  it("keeps a field the sample no longer has as a choice and says so", async () => {
    await i18n.changeLanguage("en");
    const attributes = ATTRIBUTES.attributes as Attribute[];
    show(generate({ type: "AirQualityObserved", idField: "id", attributes: { pm10: { field: "gone" } } }, attributes, "ovzdusie"));
    const pm10 = await screen.findByTestId("mapper-attribute-pm10");
    expect(within(pm10).getByRole("combobox", { name: /Source of pm10/ })).toHaveValue("field:gone");
    expect(within(pm10).getByText(/The sample has no field gone/)).toBeInTheDocument();
  });

  it("never replaces a step written by hand unless asked, and gives it back", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    show("root = this\n");
    await pickType(user);
    expect(await screen.findByText(/written by hand/)).toBeInTheDocument();
    expect(written).toEqual([]);
    await user.click(screen.getByRole("button", { name: "Map fields instead" }));
    await waitFor(() => expect(written.at(-1)?.startsWith(HEADER)).toBe(true));
    await user.click(await screen.findByRole("button", { name: "Restore the hand-written code" }));
    expect(written.at(-1)).toBe("root = this\n");
  });

  it("leaves the end time to the Lifetime step: the step never writes expiresAt (T-4842)", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    show();
    await pickType(user);
    await screen.findByTestId("mapper-attribute-dateObserved");
    expect(screen.queryByTestId("mapper-expiry")).toBeNull();
    expect(written.at(-1)).not.toContain("expiresAt");
  });

  /** A step as T-4822 wrote it: the map and its own `expiresAt` lines, in the header. */
  const legacy = (expires: { field?: string; plus?: string }, attributes: Record<string, { field: string }>) =>
    generate(
      { type: "AirQualityObserved", idField: "id", attributes, expires },
      ATTRIBUTES.attributes as Attribute[],
      "ovzdusie",
    );

  it("moves a T-4822 header expiry into spec.lifetime on open and says it is saved with the next proposal", async () => {
    await i18n.changeLanguage("en");
    const step = legacy({ field: "observed", plus: "PT10M" }, { dateObserved: { field: "observed" } });
    expect(step).toContain("root.expiresAt");
    show(step);
    expect(await screen.findByText(/now the pipeline's lifetime, in the Lifetime step/)).toBeInTheDocument();
    await waitFor(() => expect(lifetimes.at(-1)).toEqual([{ type: "AirQualityObserved", entity: { from: "dateObserved", plus: "PT10M" } }]));
    expect(written.at(-1)).not.toContain("expiresAt");
    expect(written.at(-1)?.startsWith(HEADER)).toBe(true);
  });

  it("moves a header expiry from the write time to a rolling lifetime", async () => {
    await i18n.changeLanguage("en");
    show(legacy({ plus: "PT1H" }, { dateObserved: { field: "observed" } }));
    await waitFor(() => expect(lifetimes.at(-1)).toEqual([{ type: "AirQualityObserved", entity: { rolling: "PT1H" } }]));
    expect(written.at(-1)).not.toContain("expiresAt");
  });

  it("keeps a header expiry whose field no date-time attribute holds until the person chooses", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    show(legacy({ field: "observed", plus: "PT10M" }, { status: { field: "status" } }));
    expect(await screen.findByText(/from the record field observed, and no date-time attribute is mapped/)).toBeInTheDocument();
    expect(lifetimes).toEqual([]);
    expect(written.every((step) => step.includes("root.expiresAt"))).toBe(true);
    await user.click(screen.getByRole("button", { name: "Drop the step's own expiresAt" }));
    await waitFor(() => expect(written.at(-1)).not.toContain("expiresAt"));
    expect(lifetimes.at(-1)).toBeUndefined();
  });

  it("has nothing axe finds", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    const { container } = show();
    await pickType(user);
    await screen.findByTestId("mapper-attribute-dateObserved");
    const found = await axe.run(container, { rules: { region: { enabled: false } } });
    expect(found.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`)).toEqual([]);
  });
});
