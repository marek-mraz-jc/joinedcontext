/** T-7589: coverage for PortalsArea error states, form interactions, and multi-org views. */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import en from "../src/locales/en.json";
import i18n from "../src/i18n";
import { draftOf, specOf } from "../src/pages/portals/PortalsArea";
import type { PortalSpec } from "../src/pages/portals/PortalsArea";
import { jsonResponse, renderRoute } from "./pageHarness";

const MULTI_HOSTS = {
  items: [
    { organization: "bbsk", hosts: ["doprava.bbsk.sk"], opendata: "https://opendata.bbsk.sk", sites: [], bases: [] },
    { organization: "ke", hosts: ["data.ke.sk"], opendata: "https://opendata.ke.sk", sites: [], bases: [] },
  ],
};

const SINGLE_HOST = {
  items: [
    { organization: "bbsk", hosts: ["doprava.bbsk.sk"], opendata: "https://opendata.bbsk.sk", sites: [], bases: [] },
  ],
};

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ rule: { kinds: ["Portal"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }],
};

const SPEC: PortalSpec = {
  organizationRef: "bbsk",
  path: "mobility",
  title: { en: "Mobility", sk: "Doprava" },
  languages: ["en", "sk"],
  sources: [{ name: "s1" }],
};

describe("PortalsArea coverage pass 2", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("handles specOf draft.profiles setting", () => {
    const draft = draftOf(SPEC, "bbsk");
    const spec = specOf({ ...draft, profiles: ["dcat-ap-3.0.1"] }, SPEC);
    expect(spec.profiles).toEqual(["dcat-ap-3.0.1"]);
  });

  it("handles PortalForm success change notice and back button, general refusal, and form inputs", async () => {
    const user = userEvent.setup();
    let putCount = 0;

    await renderRoute({
      path: "/portals/bbsk-mobility",
      permissions: ADMIN,
      answer: (path, request) => {
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(SINGLE_HOST);
        if (path === "/api/v1/projects/org/portals/bbsk-mobility" && request.method === "GET") {
          return jsonResponse({
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Portal",
            metadata: { name: "bbsk-mobility", namespace: "org" },
            spec: SPEC,
          });
        }
        if (path === "/api/v1/projects/org/portals/bbsk-mobility" && request.method === "PUT") {
          putCount += 1;
          if (putCount === 1) {
            return new Response(JSON.stringify({ status: 500, detail: "General lock timeout" }), {
              status: 500,
              headers: { "Content-Type": "application/problem+json" },
            });
          }
          if (request.url.includes("dryRun=All")) {
            return jsonResponse({ valid: true, verdict: { ok: true } });
          }
          return jsonResponse({
            metadata: { name: "chg-mobility-1", namespace: "org" },
            spec: {},
            status: { phase: "PendingApproval" },
          });
        }
        return undefined;
      },
    });

    const instanceInput = await screen.findByLabelText(en.portals.field.instanceName);
    fireEvent.change(instanceInput, { target: { value: "Mobility Portal" } });

    const primaryInput = screen.getByLabelText(en.portals.field.primary);
    fireEvent.change(primaryInput, { target: { value: "#0055aa" } });

    const csLangCheck = screen.getByRole("checkbox", { name: "Czech" });
    fireEvent.click(csLangCheck);

    const domainRadio = screen.getByRole("radio", { name: en.portals.mode.domain });
    fireEvent.click(domainRadio);

    const domainSelect = screen.getByLabelText(new RegExp(`^${en.portals.field.host}`));
    fireEvent.change(domainSelect, { target: { value: "doprava.bbsk.sk" } });

    const keepSlugCheck = screen.getByRole("checkbox", { name: en.portals.field.keepSlug });
    fireEvent.click(keepSlugCheck);

    const saveBtn = screen.getByRole("button", { name: en.portals.editor.save });
    await user.click(saveBtn);

    expect(await screen.findByText("General lock timeout")).toBeInTheDocument();

    await user.click(saveBtn);

    const backBtn = await screen.findByRole("button", { name: en.portals.editor.backToList });
    expect(backBtn).toBeInTheDocument();
    await user.click(backBtn);

    await waitFor(() => {
      expect(window.location.pathname).toBe("/portals");
    });
  });

  it("handles organization select in new portal form", async () => {
    await renderRoute({
      path: "/portals/new",
      permissions: ADMIN,
      answer: (path) => {
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(MULTI_HOSTS);
        if (path === "/api/v1/projects/org/portals") {
          return jsonResponse({
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "List",
            items: [],
          });
        }
        return undefined;
      },
    });

    const orgSelect = await screen.findByLabelText(new RegExp(`^${en.portals.field.organization}`));
    fireEvent.change(orgSelect, { target: { value: "ke" } });
    expect(orgSelect).toHaveValue("ke");
  });
});
