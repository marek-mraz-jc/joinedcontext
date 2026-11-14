// The Portals area (ODP-15, T-6875): the portal list and editor in their states, the host
// dropdown fed by the server's filtered list, and a refusal landing on the field it names.
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { draftOf, faultsOf, fieldOfRefusal, portalAddress, specOf } from "../src/pages/portals/PortalsArea";
import { expectAxeClean, expectOneH1, jsonResponse, list, problem, renderRoute } from "./pageHarness";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const HOSTS = {
  items: [{ organization: "bbsk", hosts: ["doprava.bbsk.sk"], opendata: "https://opendata.bbsk.sk" }],
};

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ rule: { kinds: ["Portal"], verbs: ["read", "propose", "approve", "delete"] }, organization: "bbsk" }],
};

function portal(path: string, extra: Record<string, unknown> = {}) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Portal",
    metadata: { name: `bbsk-${path}`, namespace: "org" },
    spec: { organizationRef: "bbsk", path, title: { en: path === "default" ? "Open data" : "Mobility" }, ...extra },
  };
}

describe("portal helpers", () => {
  it("names the slug, default and domain addresses", () => {
    const base = "https://opendata.bbsk.sk";
    expect(portalAddress({ organizationRef: "bbsk", path: "default" }, base)).toBe(`${base}/`);
    expect(portalAddress({ organizationRef: "bbsk", path: "mobility" }, base)).toBe(`${base}/-/mobility/`);
    expect(portalAddress({ organizationRef: "bbsk", path: "mobility", host: "doprava.bbsk.sk" }, base)).toBe("https://doprava.bbsk.sk/");
    expect(portalAddress({ organizationRef: "bbsk", path: "mobility" }, undefined)).toBeUndefined();
  });

  it("keeps the stored sources and hidden datasets and writes the slug choice with a domain", () => {
    const stored = { organizationRef: "bbsk", path: "mobility", sources: [{ name: "ke", type: "dcat-ap" }], hidden: ["x"] };
    const draft = { ...draftOf(stored, "bbsk"), title: { sk: "Doprava" }, mode: "domain" as const, host: "doprava.bbsk.sk", keepSlug: true };
    const spec = specOf(draft, stored);
    expect(spec).toMatchObject({ sources: stored.sources, hidden: ["x"], host: "doprava.bbsk.sk", slug: "keep", title: { sk: "Doprava" } });
    const back = specOf({ ...draft, mode: "slug" }, spec);
    expect(back.host).toBeUndefined();
    expect(back.slug).toBeUndefined();
  });

  it("refuses a bad name, default, a taken name, no title and a domain not chosen", () => {
    const draft = { ...draftOf(undefined, "bbsk"), path: "Mobility!" };
    expect(faultsOf(draft, true, []).path).toBe("portals.fault.path");
    expect(faultsOf({ ...draft, path: "default" }, true, []).path).toBe("portals.fault.pathDefault");
    expect(faultsOf({ ...draft, path: "skoly" }, true, ["skoly"]).path).toBe("portals.fault.pathTaken");
    expect(faultsOf({ ...draft, path: "skoly" }, true, []).title).toBe("portals.fault.title");
    expect(faultsOf({ ...draft, path: "skoly", mode: "domain" }, true, []).host).toBe("portals.fault.host");
    expect(fieldOfRefusal("spec.host: doprava.x.sk is not listed for bbsk")).toBe("host");
    expect(fieldOfRefusal("portals.max (6) reached")).toBeUndefined();
  });
});

describe("the portal list", () => {
  it("lists each portal with its address and sources", async () => {
    const { container } = await renderRoute({
      path: "/portals",
      permissions: ADMIN,
      answer: (path) => {
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
        if (path === "/api/v1/projects/org/portals") return jsonResponse(list([portal("default"), portal("mobility", { sources: [{ name: "ke", type: "dcat-ap" }] })]));
        return undefined;
      },
    });
    await expectOneH1("Open data portals");
    const table = await screen.findByRole("table", { name: "Open data portals" });
    expect(within(table).getByRole("link", { name: "Mobility" })).toHaveAttribute("href", "/portals/bbsk-mobility");
    expect(within(table).getByRole("link", { name: /opendata\.bbsk\.sk\/-\/mobility\// })).toBeInTheDocument();
    expect(within(table).getByText("1")).toBeInTheDocument();
    expect(within(table).getByText("Default")).toBeInTheDocument();
    await expectAxeClean(container);
  });

  it("says what to do when the Organization has no portal", async () => {
    await renderRoute({
      path: "/portals",
      permissions: ADMIN,
      answer: (path) => (path === "/api/v1/organization/portal-hosts" ? jsonResponse(HOSTS) : undefined),
    });
    expect(await screen.findByText("No portal yet")).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: "New portal" }).length).toBeGreaterThan(0);
  });

  it("tells somebody who makes no portal whose page it is, and fetches no portal", async () => {
    const rendered = await renderRoute({
      path: "/portals",
      answer: (path) => (path === "/api/v1/organization/portal-hosts" ? problem(403, "only an Organization's administrator") : undefined),
    });
    expect(await screen.findByText(/made by an Organization's administrators/)).toBeInTheDocument();
    expect(rendered.calls()).not.toContain("GET /api/v1/projects/org/portals");
  });
});

describe("the portal editor", () => {
  it("answers a portal of another Organization as not there", async () => {
    await renderRoute({
      path: "/portals/praha-skoly",
      permissions: ADMIN,
      answer: (path) => {
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
        if (path === "/api/v1/projects/org/portals/praha-skoly") return problem(404, "not found");
        return undefined;
      },
    });
    expect(await screen.findByText("No portal praha-skoly in your Organizations")).toBeInTheDocument();
  });

  it("offers only the listed domains and puts the server's refusal on the domain field", async () => {
    const user = userEvent.setup();
    const sent: string[] = [];
    await renderRoute({
      path: "/portals/new",
      permissions: ADMIN,
      answer: (path, request) => {
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
        if (path === "/api/v1/projects/org/portals" && request.method === "POST") {
          sent.push(request.url);
          return problem(422, "spec.host: doprava.bbsk.sk is not listed for bbsk");
        }
        return undefined;
      },
    });
    await screen.findByRole("heading", { level: 1, name: "New portal" });
    await user.type(await screen.findByLabelText(/Name in the address/), "mobility");
    await user.type(screen.getByLabelText(/Title in Slovak/), "Doprava");
    await user.click(screen.getByRole("radio", { name: "A domain of its own" }));
    const host = screen.getByLabelText(/^Domain/) as HTMLSelectElement;
    expect(Array.from(host.options).map((option) => option.value)).toEqual(["", "doprava.bbsk.sk"]);
    fireEvent.change(host, { target: { value: "doprava.bbsk.sk" } });
    await user.click(screen.getByRole("button", { name: "Propose the portal" }));
    await waitFor(() => expect(sent.length).toBe(1));
    expect(sent[0]).toContain("dryRun=All");
    const field = await screen.findByText("doprava.bbsk.sk is not listed for bbsk");
    expect(field.closest("[id]")).not.toBeNull();
  });

  it("stops a missing title before anything is sent", async () => {
    const user = userEvent.setup();
    const rendered = await renderRoute({
      path: "/portals/new",
      permissions: ADMIN,
      answer: (path) => (path === "/api/v1/organization/portal-hosts" ? jsonResponse(HOSTS) : undefined),
    });
    await screen.findByRole("heading", { level: 1, name: "New portal" });
    await user.type(await screen.findByLabelText(/Name in the address/), "mobility");
    await user.click(screen.getByRole("button", { name: "Propose the portal" }));
    expect(await screen.findByText("Write the title in the first language.")).toBeInTheDocument();
    expect(rendered.calls().filter((call) => call.startsWith("POST"))).toEqual([]);
  });

  it("does not offer to remove the default portal", async () => {
    await renderRoute({
      path: "/portals/bbsk-default",
      permissions: ADMIN,
      answer: (path) => {
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
        if (path === "/api/v1/projects/org/portals/bbsk-default") return jsonResponse(portal("default"));
        return undefined;
      },
    });
    expect(await screen.findByText(/The default portal stays/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Remove portal/ })).toBeNull();
  });
});
