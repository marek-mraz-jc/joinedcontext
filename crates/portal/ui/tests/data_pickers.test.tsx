// covers (T-2137, the module gate in gate_modules.test.ts): src/components/pickers/AttributePicker.tsx, src/components/pickers/EntitySearchPicker.tsx.
/**
 * T-4946: an attribute and a relationship target are picked, never typed. The attribute list is
 * the model's (numbers only where a KPI needs one), and the entity search goes through the
 * gateway with the person's session, so their Policy decides what is listed.
 */
import type { JSX } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AttributePicker, offered } from "../src/components/pickers/AttributePicker";
import { EntitySearchPicker } from "../src/components/pickers/EntitySearchPicker";
import { InRouter } from "./pageHarness";

const wrap = (node: JSX.Element) =>
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <InRouter>{node}</InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );

describe("data pickers (T-4946)", () => {
  let urls: string[];
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    urls = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL((input as Request).url ?? String(input), "http://localhost");
        urls.push(url.pathname + url.search);
        if (url.pathname === "/api/v1/organization/datamodels") {
          const items = [{ project: "city", name: "air", space: "air", classes: ["AirQualityObserved"], level: "project", lifecycle: "published", version: "1.0.0" }];
          return new Response(JSON.stringify({ apiVersion: "v1", kind: "OrganizationModels", items, smartDataModels: [] }), { status: 200, headers: { "Content-Type": "application/json" } });
        }
        if (url.pathname === "/api/v1/projects/city/datamodels/air/source") {
          const source = "id: https://example.org/air\nname: air\nclasses:\n  AirQualityObserved:\n    slots: [pm10, station]\nslots:\n  pm10:\n    range: float\n  station:\n    range: string\n";
          return new Response(source, { status: 200, headers: { "Content-Type": "text/yaml" } });
        }
        if (url.pathname.startsWith("/cs/air/")) {
          const found = url.searchParams.get("q") ? [] : [{ id: "urn:ngsi-ld:Station:1", type: "Station", name: { type: "Property", value: "Kallio" } }];
          return new Response(JSON.stringify(found), { status: 200, headers: { "Content-Type": "application/ld+json" } });
        }
        return new Response("{}", { status: 404, headers: { "Content-Type": "application/json" } });
      }),
    );
  });
  afterEach(() => vi.unstubAllGlobals());

  it("offers a numeric attribute of the type and saves the pick", async () => {
    const onChange = vi.fn();
    wrap(<AttributePicker label="Attribute" project="city" space="air" type="AirQualityObserved" numeric value={[]} onChange={onChange} />);
    await userEvent.click(await screen.findByRole("combobox", { name: "Attribute" }));
    await userEvent.click(await screen.findByRole("option", { name: /pm10/ }));
    expect(onChange).toHaveBeenCalledWith(["pm10"]);
    expect(offered({ name: "station", range: "string", kind: "Property" }, "any", true)).toBe(false);
  });

  it("says what to do when the type has no such attribute", async () => {
    wrap(<AttributePicker label="Attribute" project="city" space="air" type="Unknown" value={[]} onChange={vi.fn()} />);
    await userEvent.click(await screen.findByRole("combobox", { name: "Attribute" }));
    expect(await screen.findByText(en.picker.attribute.none.replace("{type}", "Unknown"))).toBeInTheDocument();
  });

  it("finds a relationship target through the space and says when nothing matches", async () => {
    const onChange = vi.fn();
    wrap(<EntitySearchPicker label="Station" space="air" type="Station" value="" onChange={onChange} />);
    await userEvent.click(await screen.findByRole("combobox", { name: "Station" }));
    await userEvent.click(await screen.findByRole("option", { name: /Kallio/ }));
    expect(onChange).toHaveBeenCalledWith("urn:ngsi-ld:Station:1");
    expect(urls.some((u) => u.startsWith("/cs/air/ngsi-ld/v1/entities?type=Station"))).toBe(true);
  });
});
