/**
 * T-5082: the catalogue's script mounts the Endpoint data view only for an `https://api.{domain}`
 * origin and a slug; anything else in explore.html's address is refused before a request leaves.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { mount } from "../src/explorer/main";
import i18n from "../src/explorer/i18n";
import { Map as NoMap } from "../src/explorer/no-map";

describe("the catalogue widget's mount", () => {
  it("refuses an origin that is not an api host and a slug that is no Endpoint's", () => {
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    const element = document.createElement("div");
    for (const origin of ["http://api.example.test", "https://evil.test", "https://api.example.test/x", "https://user@api.example.test"]) {
      expect(() => mount(element, { origin, slug: "air" })).toThrow(/origin/);
    }
    for (const slug of ["", "../x", "a/b", "A".repeat(65)]) {
      expect(() => mount(element, { origin: "https://api.example.test", slug })).toThrow(/slug/);
    }
    expect(fetch).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it("carries only the view's strings, in English and Slovak, and no map", () => {
    expect(Object.keys(i18n.options.resources ?? {}).sort()).toEqual(["en", "sk"]);
    expect(Object.keys(i18n.options.resources?.sk.translation ?? {}).sort()).toEqual(["app", "endpoints", "entityGrid", "spaces"]);
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
