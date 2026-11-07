/**
 * T-4156: the UX photo audit's camera. A shot is named by the audit's rule and recorded beside the
 * PNG, the contact sheet groups the PNGs by route, and a finding without a severity is refused.
 */
// covers: e2e/live/shoot.ts
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import { describe, expect, it } from "vitest";
import { findingErrors, shot, shotName, writeIndex } from "../e2e/live/shoot";

/** A page whose screenshot writes a stand-in PNG and remembers the options it was given. */
function fakePage(width: number) {
  const options: Record<string, unknown>[] = [];
  const page = {
    viewportSize: () => ({ width, height: 812 }),
    locator: (selector: string) => ({ selector }),
    getByText: (text: RegExp) => ({ text: String(text) }),
    screenshot: async (option: { path: string }) => {
      options.push(option);
      writeFileSync(option.path, "png");
    },
  } as unknown as Page;
  return { page, options };
}

const finding = {
  file: "portal-projects-project-spaces-empty-viewer-sk-375.png",
  route: "/projects/$project/spaces",
  width: 375,
  lang: "sk",
  who: "viewer",
  rule: "visual",
  severity: "major",
  description: "The table scrolls sideways on a phone.",
  fix: "Stack the columns below 768 px.",
};

describe("shot", () => {
  it("names a file by area, route, state, person, language and width", () => {
    expect(shotName({ area: "Portal", route: "/projects/$project/spaces", state: "empty list", who: "viewer", lang: "sk", width: 375 })).toBe(
      "portal-projects-project-spaces-empty-list-viewer-sk-375.png",
    );
    expect(shotName({ area: "data", route: "/", state: "start", who: "anonymous", lang: "en", width: 1440 })).toBe(
      "data-root-start-anonymous-en-1440.png",
    );
  });

  it("does nothing without SHOTS_DIR and masks password fields and token-shaped text when it shoots", async () => {
    const { page, options } = fakePage(375);
    expect(await shot(page, { area: "portal", route: "/", state: "start", who: "steward", lang: "en" }, "")).toBeUndefined();
    expect(options).toEqual([]);
    const dir = mkdtempSync(join(tmpdir(), "shots-"));
    const file = await shot(page, { area: "portal", route: "/", state: "start", who: "steward", lang: "en" }, dir);
    expect(file).toBe(join(dir, "portal-root-start-steward-en-375.png"));
    expect(options[0]).toMatchObject({ fullPage: true, mask: [{ selector: "input[type=password]" }, { text: expect.stringContaining("eyJ") }] });
  });
});

describe("writeIndex", () => {
  it("lists every PNG once, grouped by the route each shows", async () => {
    const dir = mkdtempSync(join(tmpdir(), "shots-"));
    for (const [route, width] of [
      ["/projects/$project/spaces", 375],
      ["/organization/<people>", 768],
    ] as const) {
      await shot(fakePage(width).page, { area: "portal", route, state: "list", who: "viewer", lang: "en" }, dir);
    }
    expect(writeIndex(dir)).toBe(2);
    const html = readFileSync(join(dir, "index.html"), "utf8");
    expect(html.match(/<section>/g)).toHaveLength(2);
    expect(html).toContain("<h2>/projects/$project/spaces</h2>");
    expect(html).toContain("<h2>/organization/&lt;people&gt;</h2>");
    expect(html).toContain('src="portal-projects-project-spaces-list-viewer-en-375.png"');
    expect(html).not.toMatch(/https?:\/\//);
  });

  it("refuses a findings.json whose entry has no severity", () => {
    const dir = mkdtempSync(join(tmpdir(), "shots-"));
    const { severity: _severity, ...noSeverity } = finding;
    writeFileSync(join(dir, "findings.json"), JSON.stringify([noSeverity]));
    expect(() => writeIndex(dir)).toThrow(/findings\[0\]: severity is missing/);
  });
});

describe("findingErrors", () => {
  it("accepts a complete finding and names each wrong field", () => {
    expect(findingErrors([finding])).toEqual([]);
    expect(findingErrors([{ ...finding, severity: "cosmetic", width: 1024, extra: 1 }])).toEqual([
      "findings[0].width: 1024 is not one of 375, 768, 1440",
      "findings[0].severity: \"cosmetic\" is not one of blocker, major, minor",
      "findings[0]: extra is not a known field",
    ]);
    expect(findingErrors({})).toEqual(["findings: is object, not array"]);
  });
});
