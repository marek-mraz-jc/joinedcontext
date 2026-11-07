/**
 * T-3278: one term per concept. The navigation names a section, and every sentence that speaks of
 * the same thing uses that name: a Slovak page that says "Koncové body" in the menu and
 * "endpoint" in two hundred sentences reads like two different things.
 */
import { describe, expect, it } from "vitest";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";

type Catalogue = Record<string, unknown>;

function flat(catalogue: Catalogue, prefix = ""): [string, string][] {
  return Object.entries(catalogue).flatMap(([key, value]) =>
    typeof value === "string" ? [[`${prefix}${key}`, value] as [string, string]] : flat(value as Catalogue, `${prefix}${key}.`),
  );
}

interface Term {
  concept: string;
  /** The navigation key that names it, and the name it must have. */
  nav?: [string, string];
  /** Words for the same concept that are not its name here. */
  banned: RegExp;
  /** Keys where the banned word means something else (a Kanban board is not a dashboard). */
  elsewhere?: RegExp;
}

const TERMS: Record<string, Term[]> = {
  sk: [
    {
      concept: "endpoint",
      nav: ["nav.endpoints", "Endpointy"],
      // "rozhranie" is an interface: the NGSI-LD API of a representation is one, an endpoint is not.
      banned: /koncov\p{L}*\s+bod|koncovk|rozhran/iu,
      elsewhere: /^endpoints\.representationOption\./,
    },
    { concept: "data source", nav: ["nav.datasources", "Zdroje údajov"], banned: /zdroj\p{L}*\s+dát|dátov\p{L}*\s+zdroj/iu },
    {
      concept: "dashboard",
      nav: ["nav.dashboards", "Dashboardy"],
      banned: /nástenk/iu,
      elsewhere: /^(spaces\.views\.|whatsNew\.entries\.dataViews\.)/,
    },
    {
      // T-4219 (photo audit T-4157): the navigation said "Pipelines" and Home "Spustite kanál".
      // A "kanál" stays where it is a feed (GTFS Realtime), a channel or a way to send something.
      concept: "pipeline",
      nav: ["nav.pipelines", "Pipeliny"],
      banned: /kanál|kanal|^Pipelines$/iu,
      elsewhere: /^(datasources\.|workspaces\.emptyHint$|organization\.people\.password\.|knowledge\.)/,
    },
  ],
  en: [
    { concept: "data source", nav: ["nav.datasources", "Data sources"], banned: /\bdatasources?\b/i },
    { concept: "copy", nav: ["nav.workspaces", "Copies"], banned: /\bworkspaces?\b/i, elsewhere: /^(agentRun\.|agentprofiles\.)/ },
  ],
};

const CATALOGUES: Record<string, Catalogue> = { en, sk };

describe("one term per concept (T-3278)", () => {
  for (const [lang, terms] of Object.entries(TERMS)) {
    const strings = flat(CATALOGUES[lang]);
    for (const term of terms) {
      it(`${lang}: ${term.concept} has one name`, () => {
        if (term.nav) {
          expect(strings.find(([key]) => key === term.nav?.[0])?.[1]).toBe(term.nav[1]);
        }
        const other = strings.filter(([key, text]) => term.banned.test(text) && !term.elsewhere?.test(key));
        expect(other, `use "${term.nav?.[1] ?? term.concept}"`).toEqual([]);
      });
    }
  }

  it("goes red on another word for a named concept", () => {
    const term = TERMS.sk[0];
    expect(term.banned.test("Všetky koncové body")).toBe(true);
    expect(term.banned.test("Všetky endpointy")).toBe(false);
  });
});
