// T-7706: the output mapper says a field that holds no boolean cannot feed a boolean attribute.
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { OutputMapper } from "../src/pages/pipelines/OutputMapper";
import type { Expiry, Lifetime } from "../src/pages/pipelines/lifetime";

const ATTRIBUTES = {
  model: "bb-air-quality",
  version: "1.0.0",
  type: "AirQualityObserved",
  description: "One air-quality station and its latest hourly means.",
  attributes: [
    { name: "dateObserved", kind: "Property", valueType: "string", format: "date-time", required: true },
    { name: "pm10", kind: "Property", valueType: "number", required: false, unit: { code: "GQ", ucum: "ug/m3" } },
    { name: "active", kind: "Property", valueType: "boolean", required: false },
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


describe("the output mapper, a boolean attribute", () => {
  it("says what the sample holds when a field that is no boolean is mapped to it", async () => {
    await i18n.changeLanguage("en");
    const user = userEvent.setup();
    show();
    await pickType(user);
    const active = await screen.findByTestId("mapper-attribute-active");
    await user.selectOptions(within(active).getByRole("combobox", { name: /Source of active/ }), "field:status");
    expect(await within(active).findByText(/status holds “working”, which is not true or false; active takes true or false/)).toBeInTheDocument();
  });
});
