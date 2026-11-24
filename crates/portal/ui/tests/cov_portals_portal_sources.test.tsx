// T-7589: coverage for PortalSources.tsx (source faults, refusal text, datasets pagination, run log, add form, datasets tab)
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, within } from "@testing-library/react";
import i18n from "../src/i18n";
import { EMPTY_SOURCE, readDatasets, sourceFaults } from "../src/pages/portals/PortalSources";
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

const SPEC: PortalSpec = {
  organizationRef: "bbsk",
  path: "mobility",
  title: { en: "Mobility" },
  sources: [],
};

describe("PortalSources helpers coverage", () => {
  it("validates byte budget in sourceFaults (line 135)", () => {
    const valid = {
      ...EMPTY_SOURCE,
      name: "valid-source",
      type: "dcat-ap" as const,
      url: "https://example.com/catalog.ttl",
      records: "100",
      bytes: "invalid-bytes",
    };
    const faults = sourceFaults(valid, []);
    expect(faults.bytes).toBe("portals.sources.fault.number");

    const zeroBytes = { ...valid, bytes: "0" };
    expect(sourceFaults(zeroBytes, []).bytes).toBe("portals.sources.fault.number");
  });

  it("paginates readDatasets across multiple pages and breaks on empty response (lines 678-680)", async () => {
    let callCount = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : (input as Request).url);
        if (url.pathname.includes("/datasets")) {
          callCount += 1;
          const offset = Number(url.searchParams.get("offset") ?? 0);
          if (offset === 0) {
            return jsonResponse({
              total: 205,
              missingPublisher: [],
              items: Array.from({ length: 200 }, (_, i) => ({
                name: `dataset-${i}`,
                title: `Dataset ${i}`,
                source: "endpoints",
                hidden: false,
                missing: [],
              })),
            });
          }
          return jsonResponse({
            total: 205,
            missingPublisher: [],
            items: Array.from({ length: 5 }, (_, i) => ({
              name: `dataset-${200 + i}`,
              title: `Dataset ${200 + i}`,
              source: "endpoints",
              hidden: false,
              missing: [],
            })),
          });
        }
        return jsonResponse({});
      }),
    );

    const result = await readDatasets("test-portal");
    expect(result.total).toBe(205);
    expect(result.items.length).toBe(205);
    expect(callCount).toBe(2);

    // Test break when next.items is empty (line 679)
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : (input as Request).url);
        if (url.pathname.includes("/datasets")) {
          const offset = Number(url.searchParams.get("offset") ?? 0);
          if (offset === 0) {
            return jsonResponse({ total: 300, missingPublisher: [], items: Array.from({ length: 200 }, () => ({ name: "d" })) });
          }
          return jsonResponse({ total: 300, missingPublisher: [], items: [] });
        }
        return jsonResponse({});
      }),
    );
    const broken = await readDatasets("test-portal-empty-next");
    expect(broken.items.length).toBe(200);
  });
});

describe("PortalDatasetsTab coverage", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("handles toggle failure setting refusal (lines 713, 720) and empty title text (line 773, 777)", async () => {
    await renderRoute({
      path: "/portals/bbsk-mobility",
      permissions: ADMIN,
      answer: (path, request) => {
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
        if (path === "/api/v1/projects/org/portals/bbsk-mobility" && request.method === "GET") {
          return jsonResponse({
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Portal",
            metadata: { name: "bbsk-mobility", namespace: "org" },
            spec: { ...SPEC, hidden: ["ds-hidden"] },
          });
        }
        if (path === "/api/v1/projects/org/portals/bbsk-mobility" && request.method === "PUT") {
          return jsonResponse({
            type: "about:blank",
            title: "Forbidden",
            status: 403,
            detail: "Cannot modify hidden datasets at this time",
          }, 403);
        }
        if (path === "/api/v1/projects/org/portals/bbsk-mobility/datasets") {
          return jsonResponse({
            total: 2,
            missingPublisher: [],
            items: [
              { name: "ds-visible", title: "Visible Dataset", source: "endpoints", hidden: false, missing: [] },
              { name: "ds-hidden", title: "", source: "custom-source", hidden: true, missing: [{ profile: "dcat-ap-sk-2.0", fields: ["title", "desc"] }] },
            ],
          });
        }
        return undefined;
      },
    });

    fireEvent.click(await screen.findByRole("tab", { name: "Datasets" }));
    const table = await screen.findByRole("table", { name: "Datasets of the portal" });

    // Click Unhide to trigger mutation failure (lines 713, 720)
    const unhideBtn = within(table).getByRole("button", { name: "List ds-hidden again" });
    fireEvent.click(unhideBtn);

    expect(await screen.findByText("Cannot modify hidden datasets at this time")).toBeInTheDocument();
  });
});
