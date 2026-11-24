// T-7706: a source's test answered with something the Portal cannot read says the generic words, not a stack message.
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen } from "@testing-library/react";
import { jsonResponse, renderRoute } from "./pageHarness";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const HOSTS = { items: [{ organization: "bbsk", hosts: [], opendata: "https://opendata.bbsk.sk", sites: [], bases: [] }] };
const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ rule: { kinds: ["Portal"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }],
};
const KOSICE = { name: "kosice", type: "dcat-ap", url: "https://data.kosice.sk/catalog.ttl", allowedHosts: ["data.kosice.sk"], schedule: "23 3 * * *", budget: { records: 5000, bytes: 50000000 } };
const PORTAL = "/api/v1/projects/org/portals/bbsk-mobility";

describe("a source's test, answered with nothing readable", () => {
  it("says the generic words of the Portal", async () => {
    await renderRoute({
      path: "/portals/bbsk-mobility",
      permissions: ADMIN,
      answer: (path, request) => {
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
        if (path === PORTAL && request.method === "GET") {
          return jsonResponse({
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Portal",
            metadata: { name: "bbsk-mobility", namespace: "org" },
            spec: { organizationRef: "bbsk", path: "mobility", title: { en: "Mobility" }, sources: [KOSICE] },
          });
        }
        return path.endsWith("/sources/kosice/test") ? jsonResponse(null) : undefined;
      },
    });
    fireEvent.click(await screen.findByRole("tab", { name: "Sources" }));
    fireEvent.click(await screen.findByRole("button", { name: "Test connection of kosice" }));
    expect(await screen.findByText("The Portal did not answer. Check the connection and try again.")).toBeInTheDocument();
  });
});
