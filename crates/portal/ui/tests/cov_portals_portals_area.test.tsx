// T-7589: coverage for PortalsArea.tsx (retry handlers, multi-org columns, appearance cleanup, faultsOf, fieldOfRefusal, datasets tab link, change handling, form fields)
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import i18n from "../src/i18n";
import { draftOf, faultsOf, fieldOfRefusal, specOf } from "../src/pages/portals/PortalsArea";
import type { PortalDraft, PortalSpec } from "../src/pages/portals/PortalsArea";
import { jsonResponse, renderRoute } from "./pageHarness";

const MULTI_HOSTS = {
  items: [
    { organization: "bbsk", hosts: ["doprava.bbsk.sk"], opendata: "https://opendata.bbsk.sk", sites: [], bases: [] },
    { organization: "ke", hosts: ["data.ke.sk"], opendata: "https://opendata.ke.sk", sites: [], bases: [] },
  ],
};

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ rule: { kinds: ["Portal"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }],
};

describe("PortalsArea pure helpers coverage", () => {
  it("deletes appearance when both instanceName and colours are empty (line 296)", () => {
    const draft: PortalDraft = {
      organization: "bbsk",
      path: "test",
      title: { en: "Test" },
      languages: ["en"],
      mode: "slug",
      host: "",
      keepSlug: false,
      instanceName: "",
      primary: "",
      profiles: undefined,
    };
    const stored: PortalSpec = {
      organizationRef: "bbsk",
      path: "test",
      appearance: {},
    };
    const spec = specOf(draft, stored);
    expect(spec.appearance).toBeUndefined();
  });

  it("covers missing organization (line 307), empty languages (line 311), invalid primary color (line 314)", () => {
    const draft = draftOf(undefined, "");
    expect(faultsOf(draft, true, []).organization).toBe("portals.fault.organization");

    const noLang = { ...draft, organization: "bbsk", languages: [] };
    expect(faultsOf(noLang, true, []).languages).toBe("portals.fault.languages");

    const invalidHex = { ...draft, organization: "bbsk", primary: "#invalidhex" };
    expect(faultsOf(invalidHex, true, []).primary).toBe("portals.fault.primary");
  });

  it("covers fieldOfRefusal prefix matches (lines 322-326)", () => {
    expect(fieldOfRefusal("spec.organizationRef: error")).toBe("organization");
    expect(fieldOfRefusal("spec.title.sk: error")).toBe("title");
    expect(fieldOfRefusal("spec.languages[0]: error")).toBe("languages");
    expect(fieldOfRefusal("spec.appearance.colours.primary: error")).toBe("primary");
    expect(fieldOfRefusal("spec.profiles[0]: error")).toBe("profiles");
    expect(fieldOfRefusal("metadata.name: taken")).toBe("path");
    expect(fieldOfRefusal("unrelated.field: error")).toBeUndefined();
  });
});

describe("PortalEditor coverage", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("handles switching from Datasets empty state to Sources tab via onSources (line 381)", async () => {
    const user = userEvent.setup();
    await renderRoute({
      path: "/portals/bbsk-mobility",
      permissions: ADMIN,
      answer: (path) => {
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(MULTI_HOSTS);
        if (path === "/api/v1/projects/org/portals/bbsk-mobility") {
          return jsonResponse({
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Portal",
            metadata: { name: "bbsk-mobility", namespace: "org" },
            spec: { organizationRef: "bbsk", path: "mobility", title: { en: "Mobility" } },
          });
        }
        if (path === "/api/v1/projects/org/portals/bbsk-mobility/datasets") {
          return jsonResponse({ total: 0, missingPublisher: [], profiles: [], items: [] });
        }
        return undefined;
      },
    });

    fireEvent.click(await screen.findByRole("tab", { name: "Datasets" }));
    const toSourcesBtn = await screen.findByRole("button", { name: "Go to Sources" });
    await user.click(toSourcesBtn);

    // Tab switched to Sources (line 381)
    expect(await screen.findByRole("tab", { name: "Sources", selected: true })).toBeInTheDocument();
  });
});
