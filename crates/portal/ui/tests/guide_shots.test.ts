/**
 * T-3270: the live journeys' guide shots. The helper is off unless GUIDE_SHOTS=1, names a shot by
 * the guide's rule, shoots English then Slovak through the Portal's own language menu at 1280 px,
 * masks password fields, and leaves the page in English at the size it had.
 */
// covers: e2e/live/guide.ts.
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import type { Page } from "@playwright/test";
import { afterEach, describe, expect, it, vi } from "vitest";
import { GUIDE_WIDTH, guideShot, guideShotPath, guideShotsOn } from "../e2e/live/guide";

/**
 * A page that records what the helper does to it; `lang` is the document's language, `offered`
 * the languages its header menu lists.
 */
function fakePage(lang = "en", dialogOpen = false, offered = ["English", "Slovenčina"]) {
  const calls: string[] = [];
  let current = lang;
  let address = "https://portal.example/projects/p/ckan?lang=en";
  let size = { width: 1600, height: 1000 };
  const masks: unknown[] = [];
  const page = {
    evaluate: async () => current,
    viewportSize: () => size,
    setViewportSize: async (next: { width: number; height: number }) => {
      size = next;
      calls.push(`size ${next.width}`);
    },
    url: () => address,
    goto: async (url: string) => {
      address = url;
      current = new URL(url).searchParams.get("lang") ?? current;
      calls.push(`goto ${new URL(url).search}`);
    },
    keyboard: { press: async (key: string) => calls.push(`press ${key}`) },
    getByRole: (role: string, { name }: { name: string }) => ({
      count: async () => (role === "menuitem" && !offered.includes(name) ? 0 : 1),
      click: async () => {
        calls.push(`${role} ${name}`);
        if (role === "menuitem") current = name === "Slovenčina" ? "sk" : "en";
      },
      first: () => ({ click: async () => calls.push(`${role} ${name}`) }),
    }),
    waitForFunction: async (_fn: unknown, wanted: string) => calls.push(`wait ${wanted}`),
    locator: (selector: string) => Object.assign(new String(selector), { count: async () => (dialogOpen && selector.includes("dialog") ? 1 : 0) }),
    screenshot: async ({ path, mask }: { path: string; mask: unknown[] }) => {
      calls.push(`shot ${path.split("/").slice(-2).join("/")}`);
      masks.push(...mask);
      mkdirSync(dirname(path), { recursive: true });
      writeFileSync(path, current);
    },
  };
  return { page: page as unknown as Page, calls, masks, size: () => size };
}

afterEach(() => {
  vi.unstubAllEnvs();
});

describe("guide shots (T-3270)", () => {
  it("are named by the guide's rule", () => {
    expect(guideShotPath("sk", "ckan-publish-3", "out")).toBe("out/sk/ckan-publish-3.png");
    for (const bad of ["", "Ckan", "a_b", "../x", "a--b", "-a", "a b"]) expect(() => guideShotPath("en", bad)).toThrow(/not lower-case/);
  });

  it("are taken only when GUIDE_SHOTS is 1", async () => {
    expect(guideShotsOn({ GUIDE_SHOTS: "1" })).toBe(true);
    for (const value of [undefined, "", "0", "true"]) expect(guideShotsOn({ GUIDE_SHOTS: value })).toBe(false);
    vi.stubEnv("GUIDE_SHOTS", "");
    const { page, calls } = fakePage();
    await guideShot(page, "space-1");
    expect(calls).toEqual([]);
  });

  it("shoot English, then Slovak through the language menu, and leave the page as it was", async () => {
    vi.stubEnv("GUIDE_SHOTS", "1");
    const root = mkdtempSync(join(tmpdir(), "guide-"));
    const { page, calls, masks, size } = fakePage();
    await guideShot(page, "space-1", { root });
    expect(calls).toEqual([
      `size ${GUIDE_WIDTH}`,
      "shot en/space-1.png",
      "button Language",
      "menuitem Slovenčina",
      "wait sk",
      "shot sk/space-1.png",
      "button Jazyk",
      "menuitem English",
      "wait en",
      "size 1600",
    ]);
    expect(masks.map(String)).toEqual(["input[type=password]", "input[type=password]"]);
    expect(size()).toEqual({ width: 1600, height: 1000 });
    expect(existsSync(join(root, "en")) && existsSync(join(root, "sk"))).toBe(true);
  });

  it("reach Slovak by a reload when the menu does not offer it and the page survives one", async () => {
    vi.stubEnv("GUIDE_SHOTS", "1");
    const root = mkdtempSync(join(tmpdir(), "guide-"));
    const { page, calls } = fakePage("en", false, ["English"]);
    await guideShot(page, "ckan-1", { root, reloads: true });
    expect(calls).toEqual([
      `size ${GUIDE_WIDTH}`,
      "shot en/ckan-1.png",
      "button Language",
      "press Escape",
      "goto ?lang=sk",
      "wait sk",
      "shot sk/ckan-1.png",
      "goto ?lang=en",
      "wait en",
      "size 1600",
    ]);
    expect(readFileSync(join(root, "sk", "ckan-1.png"), "utf8")).toBe("sk");
  });

  it("give the Slovak guide the English shot of a form a reload would empty", async () => {
    vi.stubEnv("GUIDE_SHOTS", "1");
    const root = mkdtempSync(join(tmpdir(), "guide-"));
    const { page, calls } = fakePage("en", false, ["English"]);
    await guideShot(page, "space-1", { root });
    expect(calls).toEqual([`size ${GUIDE_WIDTH}`, "shot en/space-1.png", "button Language", "press Escape", "size 1600"]);
    expect(readFileSync(join(root, "sk", "space-1.png"), "utf8")).toBe("en");
  });

  it("refuse a page with a dialog open, whose header the language menu cannot reach", async () => {
    vi.stubEnv("GUIDE_SHOTS", "1");
    const { page, calls } = fakePage("en", true);
    await expect(guideShot(page, "space-1")).rejects.toThrow(/a dialog is open/);
    expect(calls).toEqual([]);
  });

  it("refuse a page that is not in English, where the journeys' words would not match", async () => {
    vi.stubEnv("GUIDE_SHOTS", "1");
    const { page } = fakePage("sk");
    await expect(guideShot(page, "space-1")).rejects.toThrow(/journeys shoot from English/);
  });
});
