/**
 * The page the assistant opened, in words (T-0746, UI-45): a section's title and the resource the
 * route names, never the address or its query.
 */
import { beforeEach, describe, expect, it } from "vitest";
import { pushTrail, TRAIL_LENGTH } from "../src/assistant/state";
import i18n from "../src/i18n";
import { pageOf } from "../src/assistant/pageOf";

const t = (key: string, values?: Record<string, string>) => i18n.t(key, values);

describe("the page the assistant opened", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("names a route whose escape is malformed by its raw segment, never throwing (T-5699)", () => {
    expect(pageOf("/projects/p/spaces/%E0%A4%A", t)).toContain("%E0%A4%A");
  });

  it("names a section and the resource the route hands over", () => {
    expect(pageOf("/projects/helsinki/endpoints", t)).toBe("Endpoints");
    expect(pageOf("/projects/helsinki/pipelines?edit=hel-news&draft=hel-news", t)).toBe("Pipelines: hel-news");
    expect(pageOf("/projects/helsinki/settings/members?grant=jana-kovacova-steward-helsinki", t)).toBe(
      "Project settings: jana-kovacova-steward-helsinki",
    );
    // An older answer's Access route lands on Project settings and is named as it (T-2606).
    expect(pageOf("/projects/helsinki/access?grant=jana-kovacova-steward-helsinki", t)).toBe(
      "Project settings: jana-kovacova-steward-helsinki",
    );
    expect(pageOf("/projects/helsinki/spaces/complete?space=city-bikes", t)).toBe("Complete this space: city-bikes");
    expect(pageOf("/projects/helsinki/spaces/city-bikes", t)).toBe("Context Spaces: city-bikes");
    expect(pageOf("/projects/helsinki/models", t)).toBe("Data models");
    expect(pageOf("/endpoints", t)).toBe("All endpoints");
  });

  it("says another page for a route it does not know, and never shows the address", () => {
    for (const route of ["/playground?x=1", "/projects/helsinki/unknown?edit=x", "/"]) {
      const said = pageOf(route, t);
      // Read inside the notice's "Opened {page}".
      expect(said).toBe("another page");
      expect(said).not.toContain("/");
    }
  });
});

describe("the trail of pages the assistant opened (UI-59)", () => {
  it("keeps the newest three, each page once", () => {
    let known: string[] = [];
    for (const route of ["/a", "/b", "/c", "/d"]) {
      known = pushTrail(known, route);
    }
    expect(known).toEqual(["/d", "/c", "/b"]);
    expect(known).toHaveLength(TRAIL_LENGTH);
  });

  it("moves a page already opened to the top instead of repeating it", () => {
    expect(pushTrail(["/b", "/a"], "/a")).toEqual(["/a", "/b"]);
  });
});
