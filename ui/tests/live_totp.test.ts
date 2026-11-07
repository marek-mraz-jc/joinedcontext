/**
 * T-5120: the live journeys answer Keycloak's TOTP form for a person the realm asks for a second
 * factor, with a code Keycloak has not seen yet.
 */
// covers: e2e/live/totp.ts
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import { describe, expect, it } from "vitest";
import { answerOtp, totp, unusedCode } from "../e2e/live/totp";

const RFC_SEED = "12345678901234567890123456789012";

/** A Keycloak page that shows the TOTP form or not, and records what was typed and clicked. */
function fakePage(otpShown: boolean) {
  const filled: [string, string][] = [];
  const clicked: string[] = [];
  const page = {
    locator: (selector: string) => ({ count: async () => (selector === "#otp" && otpShown ? 1 : 0) }),
    fill: async (selector: string, value: string) => void filled.push([selector, value]),
    click: async (selector: string) => void clicked.push(selector),
    waitForEvent: async () => undefined,
  } as unknown as Page;
  return { page, filled, clicked };
}

describe("totp", () => {
  it("matches the RFC 6238 SHA-256 vectors", () => {
    // Appendix B: T = 59 s gives 46119246 and T = 1111111109 s 68084774; six digits keep the tail.
    expect(totp(RFC_SEED, Math.floor(59 / 30))).toBe("119246");
    expect(totp(RFC_SEED, Math.floor(1111111109 / 30))).toBe("084774");
  });
});

describe("unusedCode", () => {
  it("hands out the current, next and previous step once each, then waits for the next step", async () => {
    const dir = mkdtempSync(join(tmpdir(), "totp-"));
    let clock = 60_000 * 30; // step 60, at its start
    const now = () => clock;
    const codes = [
      await unusedCode("demo.steward@hel.fi", RFC_SEED, now, dir),
      await unusedCode("demo.steward@hel.fi", RFC_SEED, now, dir),
      await unusedCode("demo.steward@hel.fi", RFC_SEED, now, dir),
    ];
    expect(codes).toEqual([totp(RFC_SEED, 60), totp(RFC_SEED, 61), totp(RFC_SEED, 59)]);
    // Every code of steps 59..61 is spent: the clock moves past the step, 62 is fresh.
    const fourth = unusedCode("demo.steward@hel.fi", RFC_SEED, () => (clock += 15_500), dir);
    expect(await fourth).toBe(totp(RFC_SEED, 62));
  });

  it("keeps each person's codes apart", async () => {
    const dir = mkdtempSync(join(tmpdir(), "totp-"));
    const now = () => 0;
    expect(await unusedCode("demo.steward@hel.fi", RFC_SEED, now, dir)).toBe(totp(RFC_SEED, 0));
    expect(await unusedCode("demo.approver@hel.fi", RFC_SEED, now, dir)).toBe(totp(RFC_SEED, 0));
  });
});

describe("answerOtp", () => {
  it("does nothing on a page without the TOTP form", async () => {
    const { page, filled } = fakePage(false);
    expect(await answerOtp(page, { user: "demo.viewer@hel.fi" })).toBe(false);
    expect(filled).toEqual([]);
  });

  it("types a six-digit code and submits it", async () => {
    const { page, filled, clicked } = fakePage(true);
    expect(await answerOtp(page, { user: `journey-${Date.now()}@hel.fi`, totp: RFC_SEED })).toBe(true);
    expect(filled[0][0]).toBe("#otp");
    expect(filled[0][1]).toMatch(/^\d{6}$/);
    expect(clicked).toEqual(["#kc-login"]);
  });

  it("names the missing seed instead of timing out on the form", async () => {
    const { page } = fakePage(true);
    await expect(answerOtp(page, { user: "demo.steward@hel.fi", totp: "" })).rejects.toThrow(/PORTAL_TOTP/);
  });
});
