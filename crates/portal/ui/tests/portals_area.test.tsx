// The Portals area (ODP-15, T-6875): the portal list and editor in their states, the host
// dropdown fed by the server's filtered list, and a refusal landing on the field it names.
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { hostOf } from "../src/pages/portals/PortalHosts";
import userEvent from "@testing-library/user-event";
import { draftOf, faultsOf, fieldOfRefusal, portalAddress, specOf } from "../src/pages/portals/PortalsArea";
import { expectAxeClean, expectOneH1, jsonResponse, list, problem, renderRoute } from "./pageHarness";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const HOSTS = {
  items: [{ organization: "bbsk", hosts: ["doprava.bbsk.sk"], opendata: "https://opendata.bbsk.sk", sites: [], bases: [] }],
};

const BASE_HOST = "bbsk.smartcity.example";

/** bbsk serving `bbsk.smartcity.example`, with one own host on its way to served. */
const OWN_HOSTS = {
  items: [
    {
      ...HOSTS.items[0],
      bases: [BASE_HOST],
      sites: [{ host: `data2.${BASE_HOST}`, state: "dns-ok", detail: "the certificate waits until 2026-11-20T10:00Z", removable: true }],
    },
  ],
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

describe("the portal hosts (T-7415)", () => {
  it("builds the host from a label and a base, or takes the whole name", () => {
    expect(hostOf("own", " Opendata2 ", BASE_HOST, "")).toBe(`opendata2.${BASE_HOST}`);
    expect(hostOf("domain", "", BASE_HOST, "Data.bbsk.sk.")).toBe("data.bbsk.sk");
  });

  it("lists an own host with its state and detail and says what to do with none", async () => {
    await renderRoute({
      path: "/portals",
      permissions: ADMIN,
      answer: (path) => {
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(OWN_HOSTS);
        if (path === "/api/v1/projects/org/portals") return jsonResponse(list([portal("default")]));
        return undefined;
      },
    });
    const panel = await screen.findByRole("heading", { level: 2, name: "Portal hosts" });
    const card = panel.closest("section, div") as HTMLElement;
    expect(await screen.findByText(`data2.${BASE_HOST}`)).toBeInTheDocument();
    expect(screen.getByText("Certificate pending")).toBeInTheDocument();
    expect(screen.getByText(/certificate waits until/)).toBeInTheDocument();
    expect(card).toBeTruthy();
    cleanup();
    await renderRoute({
      path: "/portals",
      permissions: ADMIN,
      answer: (path) => {
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(HOSTS);
        if (path === "/api/v1/projects/org/portals") return jsonResponse(list([portal("default")]));
        return undefined;
      },
    });
    expect(await screen.findByText(/verified no domain yet/)).toBeInTheDocument();
    expect(screen.queryByText(/No portal host of your own yet/)).toBeNull();
    expect(screen.getByRole("link", { name: "Open Organization settings" })).toHaveAttribute("href", "/organization/settings");
  });

  it("adds a label under the own host and shows the server's refusal on the field", async () => {
    const user = userEvent.setup();
    const sent: unknown[] = [];
    await renderRoute({
      path: "/portals",
      permissions: ADMIN,
      answer: (path, request) => {
        if (path === "/api/v1/organization/portal-hosts" && request.method === "POST") {
          void request.json().then((body: unknown) => sent.push(body));
          return problem(422, "host: the label 'api' is reserved for the platform; choose another, e.g. data");
        }
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(OWN_HOSTS);
        if (path === "/api/v1/projects/org/portals") return jsonResponse(list([portal("default")]));
        return undefined;
      },
    });
    const label = await screen.findByLabelText(/^Name/, { selector: "input[id^='portal-host-label']" });
    await user.type(label, "api");
    expect(screen.getByText(`Adds api.${BASE_HOST}`)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Add host" }));
    await waitFor(() => expect(sent).toEqual([{ organization: "bbsk", host: `api.${BASE_HOST}` }]));
    expect(await screen.findByText(/the label 'api' is reserved/)).toBeInTheDocument();
  });

  it("removes an own host only after the confirm step", async () => {
    const user = userEvent.setup();
    const removed: string[] = [];
    await renderRoute({
      path: "/portals",
      permissions: ADMIN,
      answer: (path, request) => {
        if (request.method === "DELETE") {
          removed.push(path);
          return jsonResponse({ ...OWN_HOSTS.items[0], sites: [] });
        }
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(OWN_HOSTS);
        if (path === "/api/v1/projects/org/portals") return jsonResponse(list([portal("default")]));
        return undefined;
      },
    });
    await user.click(await screen.findByRole("button", { name: `Remove host data2.${BASE_HOST}` }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/cannot be added again/)).toBeInTheDocument();
    expect(removed).toEqual([]);
    await user.click(within(dialog).getByRole("button", { name: "Remove host" }));
    await waitFor(() => expect(removed).toEqual([`/api/v1/organization/portal-hosts/data2.${BASE_HOST}`]));
  });

  it("offers the own hosts in the portal's domain dropdown, marked until served", async () => {
    const user = userEvent.setup();
    await renderRoute({
      path: "/portals/new",
      permissions: ADMIN,
      answer: (path) => {
        if (path === "/api/v1/organization/portal-hosts") return jsonResponse(OWN_HOSTS);
        return undefined;
      },
    });
    await screen.findByRole("heading", { level: 1, name: "New portal" });
    await user.click(await screen.findByRole("radio", { name: "A domain of its own" }));
    const host = screen.getByLabelText(/^Domain/) as HTMLSelectElement;
    expect(Array.from(host.options).map((option) => option.textContent)).toContain(`data2.${BASE_HOST} (not served yet)`);
  });
});
