import { describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { pageOf } from "../src/assistant/pageOf";

const t: Parameters<typeof pageOf>[1] = (key, options) =>
  String(i18n.t(key as never, options as never));

/**
 * UI-59, T-1018: the dock names the page a person reads, never the address. Every page the
 * assistant can open (T-1011) therefore has a name here; one it cannot name reads "another
 * page", which tells the person nothing about where they were taken.
 */
describe("the page a route opens, as a person reads it", () => {
  it("names every project page the assistant can navigate to", () => {
    const named: Array<[string, string]> = [
      ["/projects/helsinki/activity", en.nav.activity],
      ["/projects/helsinki/approvals", en.nav.approvals],
      ["/projects/helsinki/assistant", en.nav.assistant],
      ["/projects/helsinki/apps/bikes", en.nav.apps],
      ["/projects/helsinki/explore", en.nav.explore],
      ["/projects/helsinki/ckan", en.nav.ckan],
      ["/projects/helsinki/models", en.nav.models],
    ];
    for (const [route, title] of named) {
      const read = pageOf(route, t);
      expect(read, `${route} reads as "${read}"`).toContain(title);
      expect(read).not.toBe(en.assistant.anotherPage);
    }
  });

  it("falls back to another page for a route it does not know", () => {
    expect(pageOf("/projects/helsinki/nothing-like-this", t)).toBe(en.assistant.anotherPage);
  });
});
