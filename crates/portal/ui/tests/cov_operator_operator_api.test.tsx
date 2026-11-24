// T-7589: coverage for operatorApi.ts - useOrganizationTitles, API doors, sentence, stateTone, when
import { describe, expect, it } from "vitest";
import { HOST_ROLES, HOST_STATES, operatorKeys, sentence, stateTone, when } from "../src/pages/operator/operatorApi";
import "../src/i18n";

describe("operatorApi formatting and constants", () => {
  it("formats sentences, state tones, dates and exposes constants", () => {
    expect(sentence("waiting for certificate")).toBe("Waiting for certificate");
    expect(sentence("already capital")).toBe("Already capital");

    expect(stateTone("served")).toBe("success");
    expect(stateTone("removed")).toBe("neutral");
    expect(stateTone("dns-ok")).toBe("info");
    expect(stateTone("proposed")).toBe("info");

    expect(when("invalid-date", "en")).toBe("invalid-date");
    const iso = "2026-11-17T09:30:15Z";
    expect(when(iso, "en", false)).toContain("2026");
    expect(when(iso, "en", true)).toContain("2026");

    expect(HOST_ROLES).toContain("working");
    expect(HOST_STATES).toContain("served");
    expect(operatorKeys.organizations).toEqual(["operator", "organizations"]);
  });
});
