/**
 * The open data portal's look: the checks the browser repeats before a publish, the colour the
 * catalogue picks for text on a fill, and the manifest a revert proposes.
 */
import { describe, expect, it } from "vitest";
import { foregroundOn, lookProblems, toForm, withLook } from "../src/pages/organization/appearanceLook";
import type { LookForm } from "../src/pages/organization/appearanceLook";

const form = (change: Partial<LookForm> = {}): LookForm => ({ ...toForm(undefined), ...change });
const colours = (change: Partial<LookForm["colours"]>): LookForm["colours"] => ({ ...toForm(undefined).colours, ...change });

describe("foregroundOn", () => {
  it("picks white on a dark fill and black on a light one, three-digit colours included", () => {
    expect(foregroundOn("#000000")).toBe("white");
    expect(foregroundOn("#ffffff")).toBe("black");
    expect(foregroundOn("#fff")).toBe("black");
    expect(foregroundOn("#00f")).toBe("white");
  });

  it("falls back to white for a value that is no colour", () => {
    expect(foregroundOn("blue")).toBe("white");
    expect(foregroundOn("")).toBe("white");
  });
});

describe("lookProblems", () => {
  it("is empty for a form nobody touched", () => {
    expect(lookProblems(form())).toEqual([]);
  });

  it("names the colour field that holds no hex colour", () => {
    expect(lookProblems(form({ colours: colours({ accent: "orange", primary: "#0a0a0a" }) }))).toEqual([
      { field: "colour-accent", key: "colourInvalid" },
    ]);
  });

  it("refuses a text colour below AA on the background, with the ratio", () => {
    const found = lookProblems(form({ colours: colours({ text: "#777777", background: "#888888" }) }));
    expect(found).toHaveLength(1);
    expect(found[0]).toMatchObject({ field: "colour-text", key: "contrastLow", values: { min: 4.5 } });
  });

  it("refuses a radius that is not a whole number from 0 to 24", () => {
    for (const radius of ["25", "-1", "2.5", "wide"]) {
      expect(lookProblems(form({ radius }))).toEqual([{ field: "radius", key: "radiusInvalid", values: { max: 24 } }]);
    }
    expect(lookProblems(form({ radius: "24" }))).toEqual([]);
  });

  it("refuses an intro longer than 2048 bytes, counting bytes and not letters", () => {
    expect(lookProblems(form({ heroIntro: "a".repeat(2048) }))).toEqual([]);
    expect(lookProblems(form({ heroIntro: "ž".repeat(1100) }))).toEqual([
      { field: "hero-intro", key: "introTooLong", values: { max: 2048 } },
    ]);
  });

  it("asks for the label of a link that has an address, and for an address a link may have", () => {
    const found = lookProblems(
      form({
        navLinks: [
          { label: "", url: "https://hel.fi" },
          { label: "Empty row", url: "" },
          { label: "", url: "" },
        ],
        footerLinks: [{ label: "Odd", url: "javascript:alert(1)" }],
      }),
    );
    expect(found).toEqual([
      { field: "navLinks-0-label", key: "linkLabelMissing" },
      { field: "navLinks-1-url", key: "linkUrlInvalid" },
      { field: "footerLinks-0-url", key: "linkUrlInvalid" },
    ]);
  });
});

describe("withLook", () => {
  it("drops spec.appearance when a revert leaves no member, and keeps the other members", () => {
    const stored = {
      kind: "Organization",
      status: { phase: "Ready" },
      spec: { domain: "hel.fi", appearance: { colours: { primary: "#112233" }, radius: 4 } },
    };
    expect(withLook(stored, {})).toEqual({ kind: "Organization", spec: { domain: "hel.fi" } });
    expect(withLook({ spec: { appearance: { name: "Open Hel", font: "serif" } } }, {})).toEqual({
      spec: { appearance: { name: "Open Hel" } },
    });
  });

  it("writes the look over the old one and never sends the status back", () => {
    const next = withLook({ status: { phase: "Ready" }, spec: { appearance: { name: "Open Hel", radius: 2 } } }, { radius: 8 });
    expect(next).toEqual({ spec: { appearance: { name: "Open Hel", radius: 8 } } });
  });

  it("builds a manifest from nothing", () => {
    expect(withLook(undefined, { font: "mono" })).toEqual({ spec: { appearance: { font: "mono" } } });
  });
});
