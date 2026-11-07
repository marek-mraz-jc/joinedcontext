/**
 * T-2706, UI-86: an enum slot of the space's model is picked in the Portal's grid, never typed.
 * The model's LinkML names the values and their titles; the explorer hands them to the grid.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { InRouter } from "./pageHarness";
import { queryKeys } from "../src/api/client";
import { enumsOfModel } from "../src/components/entities/filters";
import { ExplorePage } from "../src/pages/explore/ExplorePage";
import sk from "../src/locales/sk.json";

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

const MODEL = [
  "id: https://hel.fi/models/alerts",
  "name: helsinki-alerts",
  "classes:",
  "  Alert:",
  "    slots: [id, category, note]",
  "slots:",
  "  id: {}",
  "  category: { range: AlertCategory }",
  "  note: { range: string }",
  "enums:",
  "  AlertCategory:",
  "    permissible_values:",
  "      traffic:",
  "        title: { en: Traffic, sk: Doprava }",
  "        description: Roads and public transport",
  "      weather:",
  "        title: Weather",
  "      health: {}",
  "",
].join("\n");

describe("the enums of a model", () => {
  it("lists each enum slot of the class with its values titled in the person's language", () => {
    expect(enumsOfModel(MODEL, "Alert", "sk")).toEqual({
      category: [
        { value: "traffic", title: "Doprava", description: "Roads and public transport" },
        { value: "weather", title: "Weather", description: undefined },
        { value: "health", title: undefined, description: undefined },
      ],
    });
  });

  it("gives nothing without a model, a type, or an enum", () => {
    expect(enumsOfModel(undefined, "Alert", "en")).toEqual({});
    expect(enumsOfModel(MODEL, undefined, "en")).toEqual({});
    expect(enumsOfModel(MODEL, "Nope", "en")).toEqual({});
    expect(enumsOfModel("not: [yaml", "Alert", "en")).toEqual({});
  });
});

const ENTITY = {
  id: "urn:ngsi-ld:Alert:hel.fi:helsinki:1",
  type: "Alert",
  category: { type: "Property", value: "traffic" },
  note: { type: "Property", value: "Mannerheimintie closed" },
};

function urlOf(input: unknown): string {
  return typeof input === "string" ? input : ((input as Request).url ?? "");
}

afterEach(() => {
  vi.unstubAllGlobals();
});

it("offers the enum's values as a picker in the explorer's edit cell", async () => {
  await i18n.changeLanguage("sk");
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(
    queryKeys.list("helsinki", "endpoints"),
    list([
      {
        kind: "Endpoint",
        metadata: { name: "helsinki-alerts", namespace: "helsinki" },
        spec: { contextSpaceRef: "helsinki", slug: "alerts0000000000000000000000000a", audience: "public" },
      },
    ]),
  );
  client.setQueryData(
    queryKeys.list("helsinki", "spaces"),
    list([{ kind: "ContextSpace", metadata: { name: "helsinki" }, spec: { dataModelRef: "helsinki-alerts" } }]),
  );
  client.setQueryData(
    queryKeys.list("helsinki", "datamodels"),
    list([{ kind: "DataModel", metadata: { name: "helsinki-alerts" }, spec: { classes: ["Alert"], linkml: MODEL } }]),
  );
  vi.stubGlobal(
    "fetch",
    vi.fn((input: unknown) => {
      const url = urlOf(input);
      const json = (body: unknown, headers: Record<string, string> = {}) =>
        Promise.resolve(
          new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json", ...headers } }),
        );
      if (url.includes("/entities?")) {
        return json([ENTITY], { "NGSILD-Results-Count": "1" });
      }
      // The gateway's own decisions per attribute (T-5528): both are written.
      if (url.includes("/access/attributes")) {
        return json({
          type: "Alert",
          attributes: ["category", "note"].map((name) => ({ name, read: true, write: true, delete: false, reason: "grant" })),
        });
      }
      if (url.includes("/access")) {
        return json({
          subject: { type: "user", id: "someone" },
          permissions: [{ resource: { type: "Alert" }, actions: ["queryEntity", "updateAttrs"], attributes: "*" }],
        });
      }
      return Promise.resolve(new Response("", { status: 404 }));
    }),
  );
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <ExplorePage project="helsinki" initialSpace="helsinki" initialEndpoint="helsinki-alerts" initialType="Alert" />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  const cell = (await screen.findByLabelText(`${sk.entityGrid.edit} category`)) as HTMLSelectElement;
  expect(cell.tagName).toBe("SELECT");
  expect(Array.from(cell.options, (option) => [option.value, option.text])).toEqual([
    ["traffic", "Doprava"],
    ["weather", "Weather"],
    ["health", "health"],
  ]);
  await userEvent.selectOptions(cell, "weather");
  expect(await screen.findByText(`1 ${sk.entityGrid.pending}`)).toBeInTheDocument();
  // A slot that is not an enum is still typed.
  expect((screen.getByLabelText(`${sk.entityGrid.edit} note`) as HTMLElement).tagName).toBe("INPUT");
  await i18n.changeLanguage("en");
});
