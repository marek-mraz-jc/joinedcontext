// Display tab, remaining branches: organization dataset-field labels, move down, discard, all lists reset.
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { moved } from "../src/pages/portals/portalDisplay";
import { jsonResponse, renderRoute } from "./pageHarness";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const HOSTS = { items: [{ organization: "bbsk", hosts: [], opendata: "https://opendata.bbsk.sk" }] };
const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ rule: { kinds: ["Portal", "Organization"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }],
};
const CHANGE = { metadata: { name: "chg-8", namespace: "org" }, spec: {}, status: { phase: "PendingApproval" } };
const ORGANIZATION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Organization",
  metadata: { name: "bbsk", namespace: "org" },
  spec: { datasetFields: [{ name: "funding", label: { en: "Funding source" } }, { name: "reviewed", label: { sk: "Skontrolovane" } }, { name: "plain" }] },
};
type Sent = { url: string; body: { spec: { appearance?: { display?: unknown } } } };

async function open(stored: Record<string, unknown>, sent: Sent[]) {
  await renderRoute({
    path: "/portals/bbsk-doprava",
    permissions: ADMIN,
    answer: (path, request) => {
      if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
      if (path === "/api/v1/projects/org/organizations") return jsonResponse({ kind: "List", items: [ORGANIZATION] });
      if (path === "/api/v1/projects/org/portals/bbsk-doprava" && request.method === "GET") {
        return jsonResponse({
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Portal",
          metadata: { name: "bbsk-doprava", namespace: "org" },
          spec: { organizationRef: "bbsk", path: "doprava", title: { sk: "Doprava" }, languages: ["sk"], ...stored },
        });
      }
      if (path === "/api/v1/projects/org/portals/bbsk-doprava" && request.method === "PUT") {
        return request.json().then((body: Sent["body"]) => {
          sent.push({ url: request.url, body });
          return jsonResponse(request.url.includes("dryRun") ? { valid: true, verdict: { ok: true } } : CHANGE);
        });
      }
      if (path.startsWith("/api/v1/projects/org/changes/history")) return jsonResponse({ apiVersion: "v1", kind: "List", items: [] });
      return undefined;
    },
  });
  fireEvent.click(await screen.findByRole("tab", { name: "Display" }));
}

describe("the display tab, more branches", () => {
  it("keeps the order when a row would move past either end", () => {
    const rows = ["a", "b", "c"];
    expect(moved(rows, 0, -1)).toBe(rows);
    expect(moved(rows, 2, 1)).toBe(rows);
    expect(moved(rows, 1, 1)).toEqual(["a", "c", "b"]);
  });

  it("labels the Organization's dataset fields as it names them and falls back to the key", async () => {
    await open({}, []);
    const about = (await screen.findByRole("heading", { name: "About table rows" })).closest("section") as HTMLElement;
    expect(await within(about).findByRole("checkbox", { name: "Funding source" })).toBeInTheDocument();
    expect(within(about).getByRole("checkbox", { name: "Skontrolovane" })).toBeInTheDocument();
    expect(within(about).getByRole("checkbox", { name: "plain" })).toBeInTheDocument();
  });

  it("moves a row down, then discards the edit", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    await open({}, sent);
    const sections = (await screen.findByRole("heading", { name: "Dataset page sections" })).closest("section") as HTMLElement;
    expect(screen.getByText("The display shows as published.")).toBeInTheDocument();
    const before = within(sections).getAllByRole("checkbox").map((c) => (c.closest("li")?.textContent ?? ""));
    await user.click(within(sections).getByRole("button", { name: "Move Publisher down" }));
    const after = within(sections).getAllByRole("checkbox").map((c) => (c.closest("li")?.textContent ?? ""));
    expect(after).not.toEqual(before);
    expect(screen.getByText(/Unpublished changes/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Discard" }));
    expect(screen.getByText("The display shows as published.")).toBeInTheDocument();
    expect(within(sections).getByText("Inherited from the installation default")).toBeInTheDocument();
    expect(sent).toEqual([]);
  });

  it("proposes a portal without any appearance once its only display is reset", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    await open({ appearance: { display: { dataset: { sections: [{ name: "about" }] } } } }, sent);
    const sections = (await screen.findByRole("heading", { name: "Dataset page sections" })).closest("section") as HTMLElement;
    await user.click(within(sections).getByRole("button", { name: "Reset to inherited" }));
    await user.click(screen.getByRole("button", { name: "Publish" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect("appearance" in sent[1]!.body.spec).toBe(false);
    expect(await screen.findByText(/chg-8/)).toBeInTheDocument();
  });

  it("drops only the display when the portal keeps other appearance members", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    await open({ appearance: { instanceName: "Doprava", display: { dataset: { sections: [{ name: "about" }] } } } }, sent);
    const sections = (await screen.findByRole("heading", { name: "Dataset page sections" })).closest("section") as HTMLElement;
    await user.click(within(sections).getByRole("button", { name: "Reset to inherited" }));
    await user.click(screen.getByRole("button", { name: "Publish" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent[1]?.body.spec.appearance).toEqual({ instanceName: "Doprava" });
  });
});
