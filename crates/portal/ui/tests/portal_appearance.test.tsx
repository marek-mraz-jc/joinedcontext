// The Appearance tab of a portal (T-7744): every style member with where it shows from, a reset
// to inherited per field, values checked before sending, one checked Change, refusals named.
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { contrast, inherited, sourceOf, styleFaults, valueAt, withValue } from "../src/pages/portals/portalAppearance";
import { PortalAppearanceTab } from "../src/pages/portals/PortalAppearance";
import { expectAxeClean, jsonResponse, problem, renderRoute } from "./pageHarness";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the appearance model", () => {
  it("sets and removes a nested member, dropping an emptied parent", () => {
    const set = withValue({}, "colours.link", "#1d4ed8");
    expect(set).toEqual({ colours: { link: "#1d4ed8" } });
    expect(valueAt(set, "colours.link")).toBe("#1d4ed8");
    expect(withValue(set, "colours.link", undefined)).toEqual({});
    expect(withValue({ radius: 4 }, "radius", undefined)).toEqual({});
  });

  it("reads where a member comes from, the legacy font standing for both fonts", () => {
    expect(sourceOf({ fontSize: 16 }, { width: "wide" }, "fontSize")).toBe("portal");
    expect(sourceOf({}, { width: "wide" }, "width")).toBe("organization");
    expect(sourceOf({}, {}, "width")).toBe("builtIn");
    expect(inherited({ font: "serif" }, "headingFont")).toBe("serif");
    expect(inherited({ font: "serif", bodyFont: "mono" }, "bodyFont")).toBe("mono");
  });

  it("refuses each bad value by its field, contrast below WCAG AA included", () => {
    expect(styleFaults({ colours: { link: "blue" }, fontSize: 20, width: "huge", contactEmail: "x", licenseDefault: "CC BY", city: "<b>" })).toEqual({
      "colours.link": "portals.appearance.fault.colour",
      fontSize: "portals.appearance.fault.number",
      width: "portals.appearance.fault.choice",
      contactEmail: "portals.appearance.fault.email",
      licenseDefault: "portals.appearance.fault.licence",
      city: "portals.appearance.fault.text",
    });
    expect(styleFaults({ colours: { headerText: "#777777", headerBackground: "#888888" } })).toEqual({ "colours.headerText": "portals.appearance.fault.contrast" });
    expect(styleFaults({ colours: { headerText: "#ffffff", headerBackground: "#0b5394" }, fontSize: 14, buttonStyle: "outline" })).toEqual({});
    expect(contrast("#000000", "#ffffff")).toBeCloseTo(21, 0);
  });
});

const HOSTS = { items: [{ organization: "bbsk", hosts: [], opendata: "https://opendata.bbsk.sk" }] };
const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ rule: { kinds: ["Portal", "Organization"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }],
};
const CHANGE = { metadata: { name: "chg-44", namespace: "org" }, spec: {}, status: { phase: "PendingApproval" } };
const ORGANIZATION = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Organization",
  metadata: { name: "bbsk", namespace: "org" },
  spec: { appearance: { colours: { primary: "#0b5394" }, width: "wide" } },
};

type Answer = (path: string, request: Request) => Response | Promise<Response> | undefined;
type Sent = { url: string; body: { spec: { appearance?: Record<string, unknown> } } };

async function openAppearance(stored: Record<string, unknown>, answer: Answer = () => undefined) {
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
  fireEvent.click(await screen.findByRole("tab", { name: "Appearance" }));
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

describe("the appearance tab", () => {
  it("names each field's source, sets tokens over the Organization's, resets one and proposes one Change", async () => {
    expect(PortalAppearanceTab).toBeTypeOf("function");
    const user = userEvent.setup();
    const sent: Sent[] = [];
    const { container } = await openAppearance({ appearance: { instanceName: "Doprava BBSK", radius: 4 } }, proposals(sent));
    const width = await screen.findByLabelText(/^Content width/);
    await waitFor(() => expect(within(width.closest("div[class]")?.parentElement as HTMLElement).getByText("From the Organization")).toBeInTheDocument());
    await expectAxeClean(container);
    await user.type(screen.getByLabelText(/^Header background/), "#0b5394");
    await user.type(screen.getByLabelText(/^Header text/), "#ffffff");
    fireEvent.change(screen.getByLabelText(/^Buttons/), { target: { value: "outline" } });
    fireEvent.change(screen.getByLabelText(/^Text size/), { target: { value: "17" } });
    await user.click(screen.getByRole("button", { name: "Reset Corner radius (px) to inherited" }));
    await user.click(screen.getByRole("button", { name: "Publish" }));
    await waitFor(() => expect(sent.length).toBe(2));
    expect(sent[1]?.body.spec.appearance).toEqual({
      instanceName: "Doprava BBSK",
      colours: { headerBackground: "#0b5394", headerText: "#ffffff" },
      buttonStyle: "outline",
      fontSize: 17,
    });
    expect(await screen.findByText(/chg-44/)).toBeInTheDocument();
  });

  it("refuses low contrast before sending and shows a non-admin's 403 in words", async () => {
    const user = userEvent.setup();
    const sent: Sent[] = [];
    await openAppearance({}, (path, request) =>
      path === "/api/v1/projects/org/portals/bbsk-doprava" && request.method === "PUT"
        ? (sent.push({ url: request.url, body: { spec: {} } }), problem(403, "You may not propose a change to Portal bbsk-doprava."))
        : undefined,
    );
    await user.type(await screen.findByLabelText(/^Footer background/), "#777777");
    await user.type(screen.getByLabelText(/^Footer text/), "#888888");
    await user.click(screen.getByRole("button", { name: "Publish" }));
    expect(await screen.findByText("Some fields need a fix")).toBeInTheDocument();
    expect(screen.getByText(/Too little contrast/)).toBeInTheDocument();
    expect(sent).toEqual([]);
    await user.clear(screen.getByLabelText(/^Footer text/));
    await user.type(screen.getByLabelText(/^Footer text/), "#ffffff");
    await user.clear(screen.getByLabelText(/^Footer background/));
    await user.type(screen.getByLabelText(/^Footer background/), "#0f172a");
    await user.click(screen.getByRole("button", { name: "Publish" }));
    const alert = (await screen.findByText("The appearance was refused")).closest("[role=alert]") as HTMLElement;
    expect(within(alert).getByText(/You may not propose a change to Portal bbsk-doprava/)).toBeInTheDocument();
  });
});
