// covers (T-2137, the module gate in gate_modules.test.ts): src/components/layout/Shell.tsx.
// jc-multiorg-portal: a project the person reads none of (another Organization's project opened
// on this Organization's Portal host) is a not-found page in the frame of one they read, never
// drawn with its own navigation and switcher.
import { describe, expect, it } from "vitest";
import { frameFor } from "../src/components/layout/Shell";

describe("the frame of a project page", () => {
  it("is the project's own when the person reads it, the list is loading, or it is the organization", () => {
    expect(frameFor("bratislava", ["bratislava"])).toEqual({ project: "bratislava", notFound: false });
    expect(frameFor("bratislava", undefined)).toEqual({ project: "bratislava", notFound: false });
    expect(frameFor("org", [])).toEqual({ project: "org", notFound: false });
  });

  it("is the not-found page in a readable project's frame for a project of another Organization", () => {
    expect(frameFor("helsinki", ["bratislava", "zilina"])).toEqual({ project: "bratislava", notFound: true });
  });

  it("is the bare not-found page for a person who reads no project", () => {
    expect(frameFor("helsinki", [])).toBeNull();
  });

  it("keeps a page that needs no project in a frame the person reads, when the project last worked in is gone", () => {
    expect(frameFor("gone", ["bratislava"], false)).toEqual({ project: "bratislava", notFound: false });
    expect(frameFor("gone", [], false)).toEqual({ project: "gone", notFound: false });
    expect(frameFor("gone", ["gone"], false)).toEqual({ project: "gone", notFound: false });
  });
});
