// The Display tab of a portal (T-7743): the dataset page's sections, the about rows and the list
// fields, each inherited from the Organization or the built-in page until the portal sets its own,
// reset to inherited again, published as one checked Change, refusals named.
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { displayFaults, displayOf, fieldRowsOf, moved, resolved } from "../src/pages/portals/portalDisplay";
import type { Display } from "../src/pages/portals/portalDisplay";
import { ownLists } from "../src/pages/portals/PortalDisplay";
import { expectAxeClean, jsonResponse, problem, renderRoute } from "./pageHarness";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const ORG_DISPLAY: Display = { dataset: { about: ["license", "publisher"] } };

describe("the display model", () => {
  it("resolves each list from the portal, else the Organization, else built-in, hidden names after", () => {
    const own: Display = { dataset: { sections: [{ name: "about" }, { name: "resources" }] } };
    const sections = resolved(own, ORG_DISPLAY, "sections");
    expect(sections.from).toBe("portal");
    expect(sections.rows.slice(0, 3).map((r) => [r.name, r.shown])).toEqual([
      ["about", true],
      ["resources", true],
      ["publisher", false],
    ]);
    const about = resolved(own, ORG_DISPLAY, "about");
    expect(about.from).toBe("organization");
    expect(about.rows.filter((r) => r.shown).map((r) => r.name)).toEqual(["license", "publisher"]);
    expect(resolved(undefined, undefined, "fields")).toMatchObject({ from: "builtIn" });
    expect(resolved(undefined, undefined, "fields").rows.every((r) => r.shown)).toBe(true);
    // The editor starts with only the lists the portal sets itself; the rest stay inherited.
    expect(Object.keys(ownLists(own, ORG_DISPLAY))).toEqual(["sections"]);
  });

  it("offers the Organization's own dataset fields as about rows: shown on the built-in page, hidden where a list leaves them out (T-7746)", () => {
    const fieldRows = fieldRowsOf([{ name: "funding" }, { name: "reviewed" }]);
    expect(fieldRows).toEqual(["extras.x_funding", "extras.x_reviewed"]);
    const builtIn = resolved(undefined, undefined, "about", fieldRows);
    expect(builtIn.rows.slice(-2).map((r) => [r.name, r.shown])).toEqual([
      ["extras.x_funding", true],
      ["extras.x_reviewed", true],
    ]);
    const listed = resolved(undefined, ORG_DISPLAY, "about", fieldRows);
    expect(listed.rows.filter((r) => r.name.startsWith("extras.")).map((r) => r.shown)).toEqual([false, false]);
    // doprava hides them: a portal list without the field rows publishes none of them.
    const doprava = displayOf({ about: listed.rows });
    expect(doprava?.dataset?.about).toEqual(["license", "publisher"]);
  });

  it("writes only shown rows and filled headings, and nothing for lists left inherited", () => {
    const rows = resolved(undefined, undefined, "sections").rows.map((r) =>
      r.name === "about" ? { ...r, heading: { sk: " O dátach ", en: "" } } : r.name === "map" ? { ...r, shown: false } : r,
    );
    const display = displayOf({ sections: moved(rows, 6, -1) });
    expect(display?.dataset?.sections?.map((s) => s.name)).not.toContain("map");
    expect(display?.dataset?.sections?.find((s) => s.name === "about")).toEqual({ name: "about", heading: { sk: "O dátach" } });
    expect(display?.dataset?.about).toBeUndefined();
    expect(displayOf({})).toBeUndefined();
  });

  it("refuses an empty list, a bad extra key and markup in a heading before sending", () => {
    const none = resolved(undefined, undefined, "fields").rows.map((r) => ({ ...r, shown: false }));
    expect(displayFaults({ fields: none })).toEqual({ fields: "portals.display.fault.empty" });
    expect(displayFaults({ about: [{ name: "extras.Bad Key", shown: true, heading: {} }] })).toEqual({ about: "portals.display.fault.extra" });
    expect(displayFaults({ sections: [{ name: "about", shown: true, heading: { sk: "<b>x</b>" } }] })).toEqual({ sections: "portals.display.fault.heading" });
  });
});

const HOSTS = { items: [{ organization: "bbsk", hosts: [], opendata: "https://opendata.bbsk.sk" }] };
const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ rule: { kinds: ["Portal", "Organization"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }],
};
const CHANGE = { metadata: { name: "chg-7", namespace: "org" }, spec: {}, status: { phase: "PendingApproval" } };
const ORGANIZATION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Organization",
  metadata: { name: "bbsk", namespace: "org" },
  spec: { appearance: { display: ORG_DISPLAY } },
};

type Answer = (path: string, request: Request) => Response | Promise<Response> | undefined;
type Sent = { url: string; body: { spec: { appearance?: { display?: Display; instanceName?: string } } } };

async function openDisplay(stored: Record<string, unknown>, answer: Answer = () => undefined) {
  const rendered = await renderRoute({
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
          spec: { organizationRef: "bbsk", path: "doprava", title: { sk: "Doprava" }, languages: ["sk", "en"], ...stored },
        });
      }
      if (path.startsWith("/api/v1/projects/org/changes/history")) return jsonResponse({ apiVersion: "v1", kind: "List", items: [] });
      return answer(path, request);
    },
  });
  fireEvent.click(await screen.findByRole("tab", { name: "Display" }));
  return rendered;
}

function proposals(sent: Sent[]): Answer {
  return (path, request) => {
    if (path === "/api/v1/projects/org/portals/bbsk-doprava" && request.method === "PUT") {
      return request.json().then((body) => {
        sent.push({ url: request.url, body });
        return jsonResponse(request.url.includes("dryRun") ? { valid: true, verdict: { ok: true } } : CHANGE);
      });
    }
    return undefined;
  };
}

describe("the display tab", () => {
  it("names where each list comes from, hides the map, reorders the about rows and proposes one Change", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    const { container } = await openDisplay({ appearance: { instanceName: "Doprava BBSK" } }, proposals(sent));
    const sections = (await screen.findByRole("heading", { name: "Dataset page sections" })).closest("section") as HTMLElement;
    expect(within(sections).getByText("Inherited from the installation default")).toBeInTheDocument();
    const about = screen.getByRole("heading", { name: "About table rows" }).closest("section") as HTMLElement;
    await waitFor(() => expect(within(about).getByText("Inherited from the Organization")).toBeInTheDocument());
    await expectAxeClean(container);
    await user.click(within(sections).getByRole("checkbox", { name: "Map" }));
    expect(within(sections).getByText("Set on this portal")).toBeInTheDocument();
    await user.click(within(sections).getAllByText("Change the heading")[0] as HTMLElement);
    await user.type(within(sections).getAllByRole("textbox", { name: /Heading \(SK\)/ })[0] as HTMLElement, "Mapa");
    await user.click(within(about).getByRole("button", { name: "Move Publisher up" }));
    await user.type(within(about).getByLabelText(/^Dataset extra key/), "x_kraj");
    await user.click(within(about).getByRole("button", { name: "Add row" }));
    await user.click(screen.getByRole("button", { name: "Publish" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent[0]?.url).toContain("dryRun=All");
    const appearance = sent[1]?.body.spec.appearance;
    expect(appearance?.instanceName).toBe("Doprava BBSK");
    expect(appearance?.display?.dataset?.sections?.map((s) => s.name)).not.toContain("map");
    expect(appearance?.display?.dataset?.sections?.[0]).toEqual({ name: "publisher", heading: { sk: "Mapa" } });
    expect(appearance?.display?.dataset?.about?.slice(0, 2)).toEqual(["publisher", "license"]);
    expect(appearance?.display?.dataset?.about).toContain("extras.x_kraj");
    expect(appearance?.display?.datasetList).toBeUndefined();
    expect(await screen.findByText(/chg-7/)).toBeInTheDocument();
  });

  it("resets a list the portal set back to inherited and proposes the display without it", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    await openDisplay({ appearance: { display: { dataset: { sections: [{ name: "about" }] }, datasetList: { fields: ["modified"] } } } }, proposals(sent));
    const sections = (await screen.findByRole("heading", { name: "Dataset page sections" })).closest("section") as HTMLElement;
    expect(within(sections).getByText("Set on this portal")).toBeInTheDocument();
    await user.click(within(sections).getByRole("button", { name: "Reset to inherited" }));
    expect(within(sections).getByText("Inherited from the installation default")).toBeInTheDocument();
    // The list shows what it inherits now: every built-in section ticked again.
    expect(within(sections).getByRole("checkbox", { name: "Map" })).toBeChecked();
    await user.click(screen.getByRole("button", { name: "Publish" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent[1]?.body.spec.appearance?.display).toEqual({ datasetList: { fields: ["modified"] } });
  });

  it("refuses an empty list before sending and shows a non-admin's 403 in words", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    await openDisplay({}, (path, request) =>
      path === "/api/v1/projects/org/portals/bbsk-doprava" && request.method === "PUT"
        ? (sent.push({ url: request.url, body: { spec: {} } }), problem(403, "You may not propose a change to Portal bbsk-doprava."))
        : undefined,
    );
    const fields = (await screen.findByRole("heading", { name: "Dataset list fields" })).closest("section") as HTMLElement;
    for (const name of ["Description", "Format", "Publisher", "Updated"]) await user.click(within(fields).getByRole("checkbox", { name }));
    await user.click(screen.getByRole("button", { name: "Publish" }));
    expect(within(fields).getByRole("alert")).toHaveTextContent("Show at least one");
    expect(sent).toEqual([]);
    await user.click(within(fields).getByRole("button", { name: "Reset to inherited" }));
    const sections = screen.getByRole("heading", { name: "Dataset page sections" }).closest("section") as HTMLElement;
    await user.click(within(sections).getByRole("checkbox", { name: "Map" }));
    await user.click(screen.getByRole("button", { name: "Publish" }));
    const alert = (await screen.findByText("The display was refused")).closest("[role=alert]") as HTMLElement;
    expect(within(alert).getByText(/You may not propose a change to Portal bbsk-doprava/)).toBeInTheDocument();
  });
});
