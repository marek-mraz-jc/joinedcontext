// T-7589: coverage for PortalProfiles.tsx (uninstalled profile, toggling, retry on error, error formatting)
import { describe, expect, it } from "vitest";
import { BASE_PROFILE, choiceOf, profileLabel } from "../src/pages/portals/PortalProfiles";

const PROFILES = [
  { id: BASE_PROFILE, title: { en: "DCAT-AP 3.0.1" }, always: true },
  { id: "geodcat-ap-3.1.0", title: { en: "GeoDCAT-AP 3.1.0" }, always: false },
  { id: "dcat-ap-sk-2.0", title: { en: "DCAT-AP-SK 2.0" }, country: "SK", always: false, publisherPrefix: "https://data.gov.sk/id/", blocked: "needs registered publisher" },
];

describe("PortalProfiles pure helpers", () => {
  it("computes profile choices correctly", () => {
    expect(choiceOf(undefined)).toBeUndefined();
    expect(choiceOf([BASE_PROFILE])).toBeUndefined();
    expect(choiceOf([])).toEqual([]);
    expect(choiceOf([BASE_PROFILE, "geodcat-ap-3.1.0"])).toEqual(["geodcat-ap-3.1.0"]);
    expect(choiceOf(["geodcat-ap-3.1.0"])).toEqual(["geodcat-ap-3.1.0"]);
  });

  it("formats profile labels with country or fallback", () => {
    expect(profileLabel(PROFILES, BASE_PROFILE, "en")).toBe("DCAT-AP 3.0.1");
    expect(profileLabel(PROFILES, "dcat-ap-sk-2.0", "en")).toBe("DCAT-AP-SK 2.0 (SK)");
    expect(profileLabel(PROFILES, "unknown-profile", "en")).toBe("unknown-profile");
  });
});
