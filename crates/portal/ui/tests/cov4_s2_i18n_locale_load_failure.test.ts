// T-7706: i18n.ts: a locale file that fails to load reaches the callback as a message, and English stands in.
import { describe, expect, it, vi } from "vitest";

vi.mock("../src/locales/de.json?public", () => {
  throw new Error("de locale unreachable");
});

describe("localeLoader failure", () => {
  it("hands the load error's message to the callback with no data", async () => {
    const { localeLoader } = await import("../src/i18n");
    const result = await new Promise<{ error: unknown; data: unknown }>((resolve) =>
      localeLoader.read("de", "translation", (error, data) => resolve({ error, data })),
    );
    expect(typeof result.error).toBe("string");
    expect(result.error).toContain("error when mocking a module");
    expect(result.data).toBe(false);
  });
});
