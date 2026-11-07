/**
 * The one list of sections the Portal has (T-1061, UI-01, AG-77).
 *
 * `NAV_SECTIONS` names the plural segments of `/api/v1/projects/{project}/{plural}` in sidebar
 * order, and three things read it: the sidebar draws it, the assistant's `pageOf` turns a path
 * back into the page's own name, and `Shell` decides which entry is current. A second list
 * anywhere means one of the three can name a page the others do not have, so this pins that the
 * list is the list: every entry has a label the locales carry, an icon the icon set draws, and a
 * plural the router serves.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { NAV_SECTIONS, sameSection } from "../src/components/layout/navigation";
import en from "../src/locales/en.json";

/** One key of the locale bundle, `nav.spaces` as `en.nav.spaces`. */
function translated(key: string): unknown {
  return key.split(".").reduce<unknown>(
    (node, part) => (typeof node === "object" && node !== null ? (node as Record<string, unknown>)[part] : undefined),
    en,
  );
}

describe("the sidebar's sections", () => {
  it("names a page a person can actually open", () => {
    const router = readFileSync(join(__dirname, "..", "src", "router.tsx"), "utf8");
    // Either a route of its own, or the `$plural` catch-all that serves a resource list.
    expect(router).toContain('path: "/projects/$project/$plural"');
    for (const section of NAV_SECTIONS) {
      expect(typeof section.plural).toBe("string");
      expect(section.plural).not.toBe("");
    }
  });

  it("says what every entry is called", () => {
    for (const section of NAV_SECTIONS) {
      expect(typeof translated(section.labelKey)).toBe("string");
    }
  });

  it("draws an icon the icon set holds", () => {
    // `PATHS` is the icon set and is private to the module — its keys are `IconName`, which the
    // section list already satisfies at compile time. Read here so a renamed icon fails the run
    // rather than only the build.
    const icons = readFileSync(join(__dirname, "..", "src", "components", "ui", "icons.tsx"), "utf8");
    for (const section of NAV_SECTIONS) {
      expect(icons).toMatch(new RegExp(`\\n\\s*${section.icon}:`));
    }
  });

  it("names each plural once, so no two entries can claim the same page", () => {
    const plurals = NAV_SECTIONS.map((section) => section.plural);
    expect(new Set(plurals).size).toBe(plurals.length);
  });

  // The owner dropped the Flows page (T-6096): its set-ups start on Data sources and "New app".
  it("has no Flows entry", () => {
    expect(NAV_SECTIONS.map((section) => section.plural)).not.toContain("flows");
  });
});

describe("where switching project goes (T-2425, UI-05)", () => {
  it("keeps the section a person is reading, for every section the sidebar names", () => {
    for (const section of NAV_SECTIONS) {
      expect(sameSection(`/projects/banskabystrica/${section.plural}`).plural).toBe(section.plural);
    }
    // And for the sections that have a route but no sidebar entry.
    for (const plural of ["explore", "models", "ckan", "import", "federation", "shared"]) {
      expect(sameSection(`/projects/banskabystrica/${plural}`).plural).toBe(plural);
    }
  });

  it("falls back to the section's list when the page named one resource", () => {
    // The name belongs to the project that was left: `air-quality` of banskabystrica is not
    // `air-quality` of helsinki, and it is usually nothing at all there.
    expect(sameSection("/projects/banskabystrica/endpoints/air-quality").plural).toBe("endpoints");
    expect(sameSection("/projects/banskabystrica/approvals/chg-0000002a").plural).toBe("approvals");
    expect(sameSection("/projects/banskabystrica/workspaces/air-v2/compare").plural).toBe("workspaces");
    expect(sameSection("/projects/banskabystrica/spaces/complete").plural).toBe("spaces");
  });

  it("lands on Spaces when the path names no section, and never builds a path of its own", () => {
    expect(sameSection("/projects/banskabystrica").plural).toBe("spaces");
    expect(sameSection("/projects/banskabystrica/").plural).toBe("spaces");
    for (const stray of ["..", "%2e%2e", "Endpoints", "end points", ".hidden", "-x"]) {
      expect(sameSection(`/projects/banskabystrica/${stray}`).plural, stray).toBe("spaces");
    }
  });
});

describe("context sources (T-4819)", () => {
  it("sit in the sidebar right under data sources", () => {
    const plurals = NAV_SECTIONS.map((section) => section.plural as string);
    expect(plurals.indexOf("csrs")).toBe(plurals.indexOf("datasources") + 1);
    expect(translated("nav.csrs")).toBe("Context sources");
  });
});
