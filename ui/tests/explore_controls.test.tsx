// T-5622: the explorer's controls in plain words. Each field says what to type with an example,
// the query language waits under "Technical details", attributes read by the model's label, an
// App's Endpoint sits under "Applications" without a slug in brackets, and an address naming a
// type the space does not have says so instead of showing an empty grid.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { InRouter } from "./pageHarness";
import { queryKeys } from "../src/api/client";
import { ExplorePage } from "../src/pages/explore/ExplorePage";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";
import cs from "../src/locales/cs.json";
import de from "../src/locales/de.json";

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const endpoint = (name: string, title: string | undefined, slug: string, app = false) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: {
    name,
    namespace: "helsinki",
    ...(title ? { title: { en: title } } : {}),
    ...(app ? { annotations: { "joinedcontext.com/generated-by": "portal/app-reconciler" } } : {}),
  },
  spec: { contextSpaceRef: "helsinki", slug, audience: "public", enabledRepresentations: ["ngsi-ld"] },
});
const ENDPOINTS = list([
  endpoint("helsinki-alerts", "Helsinki traffic alerts", "scsd2eehkx42n53z2zyd6vshfh7s7irf"),
  endpoint("app-helsinki-alerts", "Helsinki traffic alerts", "abcd2eehkx42n53z2zyd6vshfh7s7irf", true),
  endpoint("app-air-quality", undefined, "efgh2eehkx42n53z2zyd6vshfh7s7irf", true),
]);
const SPACES = list([
  { apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: "helsinki", namespace: "helsinki" }, spec: { dataModelRef: "helsinki-city" } },
]);
const MODEL = [
  "id: https://hel.fi/models/city",
  "name: helsinki-city",
  "classes:",
  "  WeatherObserved:",
  "    slots: [id, dateObserved, stewardNote]",
  "  Event:",
  "    slots: [id]",
  "slots:",
  "  id: {}",
  "  dateObserved: { range: datetime, annotations: { label: { en: Date observed } } }",
  "  stewardNote: {}",
  "",
].join("\n");
const MODELS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataModel",
    metadata: { name: "helsinki-city", namespace: "helsinki" },
    spec: { classes: ["WeatherObserved", "Event"], linkml: MODEL },
  },
]);

function open(initialType?: string) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: unknown) => {
      const url = typeof input === "string" ? input : ((input as Request).url ?? "");
      if (url.includes("/access")) {
        return Promise.resolve(new Response(JSON.stringify({ permissions: [{ actions: ["queryEntity"] }] }), { status: 200, headers: { "Content-Type": "application/json" } }));
      }
      const lists: Record<string, unknown> = { "/spaces": SPACES, "/endpoints": ENDPOINTS, "/datamodels": MODELS };
      const listed = Object.entries(lists).find(([path]) => url.endsWith(path));
      if (listed) {
        return Promise.resolve(new Response(JSON.stringify(listed[1]), { status: 200, headers: { "Content-Type": "application/json" } }));
      }
      if (url.includes("/entities")) {
        return Promise.resolve(new Response("[]", { status: 200, headers: { "Content-Type": "application/json", "NGSILD-Results-Count": "0" } }));
      }
      return Promise.resolve(new Response("", { status: 404 }));
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(queryKeys.list("helsinki", "endpoints"), ENDPOINTS);
  client.setQueryData(queryKeys.list("helsinki", "spaces"), SPACES);
  client.setQueryData(queryKeys.list("helsinki", "datamodels"), MODELS);
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <ExplorePage project="helsinki" initialSpace="helsinki" initialEndpoint="helsinki-alerts" initialType={initialType} />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the explorer's controls in plain words (T-5622)", () => {
  it("gives every control a hint with an example and keeps NGSI-LD under Technical details", async () => {
    open("WeatherObserved");
    expect(await screen.findByText(en.entities.hint.type)).toBeInTheDocument();
    expect(screen.getByLabelText("Area")).toHaveAccessibleDescription(en.entities.hint.scope);
    expect(en.entities.hint.scope).toContain("e.g. /district/1");
    expect(screen.getByText(en.entities.hint.filters)).toBeInTheDocument();
    expect(screen.getByText(en.entities.hint.attrs)).toBeInTheDocument();
    // The Endpoint picker sits on the title line without a hint sentence (owner, T-6107).
    expect(screen.getByLabelText(en.explore.endpoint)).not.toHaveAccessibleDescription();
    // The query language is behind a disclosure, closed while the rows can show the filter.
    const technical = screen.getByTestId("explore-technical");
    expect(technical).not.toHaveAttribute("open");
    expect(within(technical).getByText("Technical details")).toBeInTheDocument();
    expect(within(technical).getByLabelText("Query as text (q)")).toHaveAccessibleDescription(en.entities.hint.q);
    // Outside the disclosure no hint names the query language.
    const outside = [en.entities.hint.type, en.entities.hint.scope, en.entities.hint.filters, en.entities.hint.attrs];
    expect(outside.filter((text) => /NGSI-LD|scopeQ|\bq\b/.test(text))).toEqual([]);
  });

  it("opens Technical details when the q says something the rows cannot", async () => {
    open("WeatherObserved");
    const technical = await screen.findByTestId("explore-technical");
    const q = within(technical).getByLabelText("Query as text (q)");
    await userEvent.type(q, "a==1|b==2");
    await waitFor(() => expect(screen.getByTestId("explore-technical")).toHaveAttribute("open"));
  });

  it("names attributes by the model's label, the attribute name as its tooltip (T-6092)", async () => {
    open("WeatherObserved");
    const box = await screen.findByRole("checkbox", { name: /Date observed/ });
    expect(box).toHaveAccessibleName("Date observed");
    expect(box).toHaveAttribute("title", "dateObserved");
    // An attribute the model gives no label keeps its name.
    expect(screen.getByRole("checkbox", { name: "stewardNote" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Add filter" }));
    const attribute = screen.getByRole("combobox", { name: "Attribute" });
    expect([...attribute.querySelectorAll("option")].map((o) => o.textContent)).toEqual(["—", "Date observed · dateObserved", "stewardNote"]);
  });

  it("lists the space's own Endpoints first and an App's under Applications, without a slug", async () => {
    open();
    const select = await screen.findByLabelText(en.explore.endpoint);
    const top = [...select.children].filter((child) => child.tagName === "OPTION").map((o) => o.textContent);
    expect(top).toEqual(["—", "Helsinki traffic alerts"]);
    const apps = select.querySelector("optgroup");
    expect(apps?.getAttribute("label")).toBe("Applications");
    // One title in two groups needs no slug; an untitled App reads by its App's name.
    expect([...(apps?.querySelectorAll("option") ?? [])].map((o) => o.textContent)).toEqual(["Helsinki traffic alerts", "air-quality"]);
    expect(select.textContent).not.toContain("(");
  });

  it("says a type in the address does not exist, lists the space's types, and one click picks one", async () => {
    open("NoSuchType");
    expect(await screen.findByText("There is no type NoSuchType in this space")).toBeInTheDocument();
    expect(screen.getByText(en.explore.unknownTypeHint)).toBeInTheDocument();
    // No grid reads a type nobody has, and no access panel speaks of it.
    expect(screen.queryByText(/0 of 0/)).toBeNull();
    expect(screen.queryByText(/NoSuchType for you/)).toBeNull();
    expect(screen.queryByText(en.explore.typeInModel)).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Event" }));
    await waitFor(() => expect(screen.queryByText("There is no type NoSuchType in this space")).toBeNull());
    expect(screen.getByLabelText(/Entity type/i)).toHaveValue("Event");
  });

  it("carries every new phrase in every locale", () => {
    const keys = {
      entities: ["typesNotGranted", "technical", "technicalScope"],
      "entities.hint": ["type", "scope", "q", "filters", "attrs"],
      explore: ["endpointApps", "unknownType", "unknownTypeHint", "export"],
    } as const;
    for (const locale of [en, sk, cs, de]) {
      for (const [section, names] of Object.entries(keys)) {
        for (const name of names) {
          const group = section.split(".").reduce<unknown>((node, part) => (node as Record<string, unknown> | undefined)?.[part], locale);
          const text = (group as Record<string, unknown> | undefined)?.[name];
          expect(typeof text === "string" && text.length > 0, `${section}.${name}`).toBe(true);
        }
      }
      expect(locale.explore.unknownType).toContain("{type}");
    }
  });
});
