// T-2814: a language chosen in the address sets the document language on the first paint, not
// only on a later switch: detection runs inside init, before a listener added after it (WCAG 3.1.1).
import { afterEach, describe, expect, it, vi } from "vitest";

describe("document language", () => {
  afterEach(() => {
    window.history.replaceState(null, "", "/");
    document.documentElement.lang = "en";
    vi.resetModules();
  });

  it("follows ?lang= from the first render", async () => {
    window.localStorage.removeItem("jc-lang");
    window.history.replaceState(null, "", "/login?lang=sk");
    document.documentElement.lang = "en";
    vi.resetModules();
    const { default: i18n, i18nReady } = await import("../src/i18n");
    await i18nReady;
    expect(i18n.language).toBe("sk");
    expect(document.documentElement.lang).toBe("sk");
  });
});
