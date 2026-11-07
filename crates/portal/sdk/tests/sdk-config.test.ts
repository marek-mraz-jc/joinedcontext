import { afterEach, describe, expect, it, vi } from "vitest";
import { ConfigError, parseConfig, readConfig, visitorLanguage } from "../src/sdk/config";

describe("sdk config", () => {
  it("parses valid config with all options and defaults", () => {
    const valid = {
      slug: "helsinki",
      orgDomain: "hel.fi",
      space: "transport",
      transport: "bridge",
      endpointName: "Helsinki Bikes",
      basemap: "https://portal.example.com/style.json",
      language: "fi",
      user: { id: "user-1", name: "Alice", roles: ["admin"] },
    };

    const cfg = parseConfig(valid);
    expect(cfg.slug).toBe("helsinki");
    expect(cfg.orgDomain).toBe("hel.fi");
    expect(cfg.space).toBe("transport");
    expect(cfg.transport).toBe("bridge");
    expect(cfg.endpointName).toBe("Helsinki Bikes");
    expect(cfg.basemap).toBe("https://portal.example.com/style.json");
    expect(cfg.language).toBe("fi");
    expect(cfg.user).toEqual({ id: "user-1", name: "Alice", roles: ["admin"] });
  });

  it("names every missing or invalid field in one ConfigError", () => {
    expect(() =>
      parseConfig({
        slug: "BAD_SLUG!",
        orgDomain: "org:with:colons",
        space: "space/with/slash",
        transport: "unknown",
      }),
    ).toThrowError(/slug.*orgDomain.*space.*transport/);
  });

  it("refuses origin transport without valid appName", () => {
    expect(() =>
      parseConfig({
        slug: "valid",
        orgDomain: "example.org",
        space: "default",
        transport: "origin",
      }),
    ).toThrowError(/appName.*required/);

    expect(() =>
      parseConfig({
        slug: "valid",
        orgDomain: "example.org",
        space: "default",
        transport: "origin",
        appName: "-invalid-name-",
      }),
    ).toThrowError(/appName/);

    const ok = parseConfig({
      slug: "valid",
      orgDomain: "example.org",
      space: "default",
      transport: "origin",
      appName: "my-app",
    });
    expect(ok.appName).toBe("my-app");
  });

  it("reads config from a Document with #jc-config script tag", () => {
    const doc = document.implementation.createHTMLDocument();
    const script = doc.createElement("script");
    script.id = "jc-config";
    script.type = "application/json";
    script.textContent = JSON.stringify({
      slug: "demo",
      orgDomain: "example.org",
      space: "main",
      transport: "bridge",
    });
    doc.head.appendChild(script);

    const cfg = readConfig(doc);
    expect(cfg.slug).toBe("demo");
    expect(cfg.endpointName).toBe("demo");
    expect(cfg.language).toBe("en");
  });

  describe("the visitor's language (T-4199)", () => {
    const page = (config: Record<string, unknown>) => {
      document.getElementById("jc-config")?.remove();
      const script = document.createElement("script");
      script.id = "jc-config";
      script.type = "application/json";
      script.textContent = JSON.stringify({ slug: "demo", orgDomain: "example.org", space: "main", transport: "bridge", ...config });
      document.head.appendChild(script);
      return document;
    };
    afterEach(() => {
      window.history.replaceState(null, "", "/");
      vi.restoreAllMocks();
      document.getElementById("jc-config")?.remove();
    });

    it("takes ?lang= first, then the browser's languages, then the page's lang", () => {
      const nav = (languages: string[]) => ({ languages, language: languages[0] });
      expect(visitorLanguage({ location: { search: "?lang=sk" }, navigator: nav(["en-US"]) })).toBe("sk");
      expect(visitorLanguage({ location: { search: "" }, navigator: nav(["sk-SK", "en"]) })).toBe("sk");
      expect(visitorLanguage({ location: { search: "?lang=<x>" }, navigator: nav(["cs"]) })).toBe("cs");
      const doc = document.implementation.createHTMLDocument();
      doc.documentElement.lang = "fi";
      expect(visitorLanguage({ location: { search: "" }, navigator: nav([]), document: doc })).toBe("fi");
      expect(visitorLanguage({ location: { search: "" }, navigator: nav([]) })).toBeUndefined();
    });

    it("a config with no language speaks the visitor's, a configured one yields only to ?lang=", () => {
      vi.spyOn(window.navigator, "languages", "get").mockReturnValue(["sk-SK", "en"]);
      expect(readConfig(page({})).language).toBe("sk");
      expect(readConfig(page({ language: "en" })).language).toBe("en");
      window.history.replaceState(null, "", "/?lang=cs");
      expect(readConfig(page({ language: "en" })).language).toBe("cs");
    });
  });

  it("throws ConfigError when #jc-config element is missing", () => {
    const doc = document.implementation.createHTMLDocument();
    expect(() => readConfig(doc)).toThrow(ConfigError);
  });
});
