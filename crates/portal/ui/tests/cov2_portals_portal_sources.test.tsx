/** T-7589: coverage for PortalSources components, error handling, run logs, and datasets tab. */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import en from "../src/locales/en.json";
import i18n from "../src/i18n";
import { ApiError } from "../src/api/client";
import { linesOf } from "../src/pages/portals/PortalSources";
import type { PortalSpec } from "../src/pages/portals/PortalsArea";
import { jsonResponse, renderRoute } from "./pageHarness";

const HOSTS = {
  items: [
    {
      organization: "bbsk",
      hosts: [],
      opendata: "https://opendata.bbsk.sk",
      sites: [],
      bases: [],
      profiles: [{ id: "dcat-ap-sk-2.0", title: { en: "DCAT-AP-SK 2.0" } }],
    },
  ],
};

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ rule: { kinds: ["Portal"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }],
};

const KOSICE_SOURCE = {
  name: "kosice",
  type: "dcat-ap" as const,
  url: "https://data.kosice.sk/catalog.ttl",
  allowedHosts: ["data.kosice.sk"],
  schedule: "23 3 * * *",
  budget: { records: 5000, bytes: 50000000 },
};

const SPEC: PortalSpec = {
  organizationRef: "bbsk",
  path: "mobility",
  title: { en: "Mobility" },
  sources: [KOSICE_SOURCE],
};

describe("PortalSources linesOf pure helper", () => {
  it("returns fallback for non-ApiError and detail/errors for ApiError", () => {
    expect(linesOf(new Error("Generic error"), "fallback")).toEqual(["fallback"]);
    expect(linesOf("string error", "fallback")).toEqual(["fallback"]);

    const withErrors = new ApiError(422, "Refusal", {
      type: "about:blank",
      title: "Refusal",
      status: 422,
      errors: ["spec.url: invalid address"],
    });
    expect(linesOf(withErrors, "fallback")).toEqual(["spec.url: invalid address"]);

    const withDetail = new ApiError(400, "Bad Request", {
      type: "about:blank",
      title: "Bad Request",
      status: 400,
      detail: "Malformed payload",
    });
    expect(linesOf(withDetail, "fallback")).toEqual(["Malformed payload"]);
  });
});

describe("PortalSourcesTab coverage pass 2", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("handles cancelling AddSourceForm and toggling Last runs", async () => {
    const user = userEvent.setup();
    await renderRoute({
      path: "/portals/bbsk-mobility",
      permissions: ADMIN,
      answer: (path) => {
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
        if (path === "/api/v1/projects/org/portals/bbsk-mobility") {
          return jsonResponse({
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Portal",
            metadata: { name: "bbsk-mobility", namespace: "org" },
            spec: SPEC,
          });
        }
        if (path.includes("/runs")) {
          return jsonResponse({
            items: [{ id: "r-1", state: "succeeded", startedAt: null, created: 5 }],
          });
        }
        return undefined;
      },
    });

    fireEvent.click(await screen.findByRole("tab", { name: "Sources" }));

    const addBtn = await screen.findByRole("button", { name: en.portals.sources.add });
    await user.click(addBtn);

    expect(screen.getByText(en.portals.sources.addTitle)).toBeInTheDocument();

    const cancelBtn = screen.getByRole("button", { name: en.portals.sources.cancel });
    await user.click(cancelBtn);

    expect(screen.queryByText(en.portals.sources.addTitle)).toBeNull();

    const showRunsBtn = screen.getByRole("button", { name: en.portals.sources.showRuns });
    await user.click(showRunsBtn);

    expect(await screen.findByRole("button", { name: en.portals.sources.hideRuns })).toBeInTheDocument();
    expect(screen.getByText("—")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: en.portals.sources.hideRuns }));
    expect(screen.queryByRole("table", { name: /Last runs/ })).toBeNull();
  });

  it("handles run log error state with retry", async () => {
    let runsCount = 0;
    await renderRoute({
      path: "/portals/bbsk-mobility",
      permissions: ADMIN,
      answer: (path) => {
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
        if (path === "/api/v1/projects/org/portals/bbsk-mobility") {
          return jsonResponse({
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Portal",
            metadata: { name: "bbsk-mobility", namespace: "org" },
            spec: SPEC,
          });
        }
        if (path.includes("/runs")) {
          runsCount += 1;
          if (runsCount === 1) {
            return new Response(JSON.stringify({ status: 500, detail: "Runs store down" }), {
              status: 500,
              headers: { "Content-Type": "application/problem+json" },
            });
          }
          return jsonResponse({ items: [] });
        }
        return undefined;
      },
    });

    fireEvent.click(await screen.findByRole("tab", { name: "Sources" }));

    const showRunsBtn = await screen.findByRole("button", { name: en.portals.sources.showRuns });
    fireEvent.click(showRunsBtn);

    const retryBtn = await screen.findByRole("button", { name: en.app.error.retry });
    fireEvent.click(retryBtn);

    expect(await screen.findByText(en.portals.runs.empty)).toBeInTheDocument();
  });
});
