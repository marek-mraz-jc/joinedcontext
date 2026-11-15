import { describe, expect, it } from "vitest";
import { guideUrl, opendataOf } from "../src/components/endpoints/AssistantGuides";
import type { components } from "../src/api/schema";

type Org = components["schemas"]["OrganizationAddressList"];
type Kind = components["schemas"]["AddressKind"];

describe("assistant guide links (T-7043)", () => {
  it("links each section of the open data /connector page", () => {
    expect(guideUrl("https://opendata.city.example", "claude")).toBe("https://opendata.city.example/connector#claude");
    expect(guideUrl("https://smartcity.sk/sk_bbsk/", "other")).toBe("https://smartcity.sk/sk_bbsk/connector#other");
    expect(guideUrl("https://smartcity.sk/sk_bbsk?x=1#top", "grok")).toBe("https://smartcity.sk/sk_bbsk/connector#grok");
  });

  it("links nothing off http(s)", () => {
    expect(guideUrl("javascript:alert(1)", "claude")).toBeUndefined();
    expect(guideUrl("not a url", "claude")).toBeUndefined();
  });

  it("takes the first open data address the registry lists", () => {
    const org = (name: string, addresses: { kind: Kind; url: string }[]): Org => ({ organization: name, title: name, addresses });
    expect(opendataOf(undefined)).toBeUndefined();
    expect(opendataOf({ items: [org("a", [{ kind: "portal", url: "https://portal.a.example/" }])] })).toBeUndefined();
    expect(
      opendataOf({
        items: [
          org("a", [{ kind: "mcp", url: "https://mcp.a.example/" }]),
          org("b", [{ kind: "opendata", url: "https://opendata.b.example/" }]),
        ],
      }),
    ).toBe("https://opendata.b.example/");
  });
});
