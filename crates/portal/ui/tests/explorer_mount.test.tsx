/**
 * T-5082: the catalogue's script mounts the Endpoint data view only for a gateway origin (any https
 * origin, or http on loopback: jc names its own) and a slug; anything else is refused before a
 * request leaves.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { mount } from "../src/explorer/main";
import i18n from "../src/explorer/i18n";
import { Map as NoMap } from "../src/explorer/no-map";

describe("the catalogue widget's mount", () => {
  it("refuses an origin that is no bare https or loopback origin and a slug that is no Endpoint's", () => {
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    const element = document.createElement("div");
    for (const origin of [
      "http://api.example.test",
      "http://localhost.evil.test",
      "http://10.0.0.1:8080",
      "https://api.example.test/x",
      "https://api.example.test/",
      "https://user@api.example.test",
      "https://API.example.test",
      "https://-x.test",
      "https://api.example.test:8443?x",
      "javascript:alert(1)",
      "",
    ]) {
      expect(() => mount(element, { origin, slug: "air" })).toThrow(/origin/);
    }
    for (const slug of ["", "../x", "a/b", "A".repeat(65)]) {
      expect(() => mount(element, { origin: "https://api.example.test", slug })).toThrow(/slug/);
    }
    expect(fetch).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it("mounts for the gateway jc names: its own origin, http on loopback, or any https origin", () => {
    // jc's viz module names its gateway at `http://localhost:42102` (the explorer refused every
    // origin but https://api.*, and the catalogue showed "the table did not start").
    vi.stubGlobal("fetch", vi.fn(() => new Promise(() => {})));
    for (const origin of ["http://localhost:42102", "http://127.0.0.1:8080", "http://[::1]:9", "https://city.example", "https://api.dev.example.test:8443"]) {
      const root = mount(document.createElement("div"), { origin, slug: "air" });
      root.unmount();
    }
    vi.unstubAllGlobals();
  });

  it("draws the page's actions as the view's buttons and hands the pressed one to the page", async () => {
    const answer = (url: URL): Response | undefined => {
      if (url.pathname.endsWith("/schema/index.json")) return Response.json({ models: [{ name: "air", version: 1 }] });
      if (url.pathname.endsWith("/schema/v1/json-schema")) return Response.json({ $defs: { Air: { properties: { co2: {} } } } });
      if (url.pathname.endsWith("/access/attributes")) return Response.json({ type: "Air", attributes: [{ name: "co2", read: true, write: false, delete: false, reason: "" }] });
      if (url.pathname.includes("/ngsi-ld/v1/")) return Response.json([]);
      return undefined;
    };
    vi.stubGlobal("fetch", vi.fn((input: RequestInfo | URL) => Promise.resolve(answer(new URL(input instanceof Request ? input.url : String(input))) ?? new Response("{}", { status: 404 }))));
    const element = document.createElement("div");
    document.body.append(element);
    const pressed: HTMLElement[] = [];
    const root = mount(element, {
      origin: "https://api.example.test",
      slug: "air",
      actions: [
        { id: "data-api-tool", label: "Data API", onClick: (button) => pressed.push(button) },
        { id: "embed-tool", label: "Embed", onClick: (button) => pressed.push(button), hasPopup: true },
      ],
    });
    await waitFor(() => expect(element.querySelector("#embed-tool")).not.toBeNull(), { timeout: 5000 });
    const embed = element.querySelector<HTMLButtonElement>("#embed-tool")!;
    expect(embed).toHaveAttribute("aria-haspopup", "dialog");
    expect(element.querySelector("#data-api-tool")).not.toHaveAttribute("aria-haspopup");
    embed.click();
    expect(pressed).toEqual([embed]);
    root.unmount();
    element.remove();
    vi.unstubAllGlobals();
  });

  it("carries only the view's strings, in English and Slovak, and no map", () => {
    expect(Object.keys(i18n.options.resources ?? {}).sort()).toEqual(["en", "sk"]);
    expect(Object.keys(i18n.options.resources?.sk.translation ?? {}).sort()).toEqual(["app", "endpoints", "entityGrid", "problem", "spaces"]);
    // A refusal speaks the page's language: the catalogue showed the API's English hint in Slovak.
    expect(i18n.getFixedT("sk")("problem.resource-not-found")).toBe("Skontrolujte názov v adrese; neexistuje alebo s vami nie je zdieľaný.");
    // The Data toolbar (T-6303) reads its labels from spaces.toolbar: the explorer showed the raw keys (T-6313).
    expect(Object.keys((i18n.options.resources?.sk.translation as { spaces: object }).spaces).sort()).toEqual(["saved", "toolbar"]);
    expect(i18n.getFixedT("sk")("spaces.toolbar.filter")).not.toBe("spaces.toolbar.filter");
    expect(i18n.getFixedT("sk")("spaces.toolbar.hide")).toBe("Skryť polia");
    expect(i18n.getFixedT("sk")("endpoints.data.full")).not.toBe(i18n.getFixedT("en")("endpoints.data.full"));
    expect(() => new NoMap()).toThrow(/no map/);
  });

  it("resolves every literal key the data view and its toolbar read (T-6319)", () => {
    const files = ["EndpointDataView", "DataToolbar", "FilterPanel", "GridToolbar", "ViewOptions"];
    const keys = new Set<string>();
    for (const file of files) {
      const text = readFileSync(join(__dirname, "../src/components/entities", `${file}.tsx`), "utf8");
      for (const m of text.matchAll(/\bt\(\s*"([a-zA-Z]+\.[a-zA-Z.]+)"/g)) keys.add(m[1]);
    }
    expect(keys.has("spaces.toolbar.filter")).toBe(true);
    for (const lng of ["en", "sk"]) {
      const missing = [...keys].filter((key) => !i18n.exists(key, { lng, fallbackLng: false }));
      expect(missing, lng).toEqual([]);
    }
  });
});
