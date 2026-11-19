/**
 * T-7413: Administration → Open data appearance. An administrator edits the open data portal's
 * look for the Organization or one of its portals, sees the home and dataset page at 1440 and
 * 400 px redrawn from the form and the contrast with the background while typing, and publishes
 * it as a Change (a checked proposal of the manifest, every other member kept) or goes back to
 * the look under it. A pair below AA and a `javascript:` link are refused before anything is
 * sent; the server's 403 says who may.
 */
import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import type { Effective } from "../src/api/permissions";
import { OpendataAppearance } from "../src/pages/organization/OpendataAppearance";
import { OrganizationPage } from "../src/pages/organization/OrganizationPage";
import { contrastRatio, foregroundOn, isLinkUrl, withLook, wrapWords } from "../src/pages/organization/appearanceLook";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [
    {
      role: "org-admin",
      binding: "admins",
      rule: { kinds: ["RoleBinding", "Organization", "Portal"], verbs: ["read", "propose", "approve", "delete"] },
    },
  ],
} as unknown as Effective;

const ORGANIZATION = {
  apiVersion: "joinedcontext.com/v1",
  kind: "Organization",
  metadata: { name: "bbsk" },
  spec: {
    domain: "bbsk.sk",
    appearance: {
      instanceName: "BBSK",
      logoSvg: "<svg xmlns='http://www.w3.org/2000/svg'/>",
      pages: [{ page: "connector", lang: "sk", markdown: "# Konektor" }],
      colours: { primary: "#0b5394", secondary: "#0f766e" },
    },
  },
  status: { phase: "Ready" },
};

const portal = (name: string, organizationRef: string, appearance?: Record<string, unknown>) => ({
  apiVersion: "joinedcontext.com/v1",
  kind: "Portal",
  metadata: { name },
  spec: { organizationRef, host: `${name}.example.sk`, ...(appearance ? { appearance } : {}) },
});

const CHANGE = {
  apiVersion: "joinedcontext.com/v1",
  kind: "ChangeProposal",
  metadata: { name: "chg-0000beef", namespace: "org" },
  spec: {},
  status: { phase: "PendingApproval", lane: "yellow" },
};

const list = (items: unknown[]) => ({ apiVersion: "v1", kind: "List", items });

interface Setup {
  write?: Response;
}

function renderAppearance(setup: Setup = {}, alone = false) {
  const sent: { method: string; path: string; dryRun: boolean; body?: Record<string, unknown> }[] = [];
  const page = alone ? <OpendataAppearance /> : <OrganizationPage tab="opendata-appearance" anchor="doprava" />;
  const result = renderPage(page, {
    path: "/organization/opendata-appearance",
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
      if (url.pathname === "/api/v1/projects/org/organizations") return json(list([ORGANIZATION]));
      if (url.pathname === "/api/v1/projects/org/portals" && request.method === "GET") {
        return json(
          list([portal("bbsk-mobility", "bbsk", { colours: { primary: "#7f1d1d" } }), portal("praha-data", "praha")]),
        );
      }
      if (url.pathname === "/api/v1/organization/opendata/pages/preview") {
        return json({ title: null, html: "<p>Údaje <strong>mesta</strong></p><script>window.ran = true</script>" });
      }
      if (url.pathname === "/api/v1/projects/org/changes") return json(list([]));
      if (url.pathname === "/api/v1/projects/org/changes/history") return json(list([]));
      if (request.method === "PUT") {
        const raw = await request.clone().text();
        const dryRun = url.searchParams.has("dryRun") || url.search.includes("dryRun");
        sent.push({ method: "PUT", path: url.pathname, dryRun, body: raw ? JSON.parse(raw) : undefined });
        if (dryRun) return json({ valid: true, verdict: { ok: true } });
        return setup.write ?? json(CHANGE, 202);
      }
      return undefined;
    },
  });
  return { ...result, sent };
}

const proposals = (sent: ReturnType<typeof renderAppearance>["sent"]) => sent.filter((item) => !item.dryRun);

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
});

describe("open data appearance: the rules jc-core holds, said in the browser", () => {
  it("measures_contrast_as_wcag_and_jc_core_do", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 5);
    expect(contrastRatio("#777777", "#ffffff")).toBeCloseTo(4.48, 2);
    expect(contrastRatio("#fff", "#ffffff")).toBeCloseTo(1, 5);
    expect(contrastRatio("red", "#ffffff")).toBeNull();
    expect(foregroundOn("#ffe977")).toBe("black");
    expect(foregroundOn("#0000bf")).toBe("white");
  });

  it("accepts_only_http_links_and_portal_paths", () => {
    for (const ok of ["https://www.bbsk.sk/", "http://a.sk/x?y=1", "/about"]) expect(isLinkUrl(ok), ok).toBe(true);
    for (const bad of ["javascript:alert(1)", "data:text/html,x", "//evil.sk", "https://u@evil.sk", "/a\"b", "https://", ""]) {
      expect(isLinkUrl(bad), bad).toBe(false);
    }
  });

  it("breaks_a_heading_into_lines_like_the_page_and_cuts_the_rest", () => {
    expect(wrapWords("Otvorené údaje Banskobystrického kraja", 16, 3)).toEqual(["Otvorené údaje", "Banskobystrického", "kraja"].map((line) => (line.length > 16 ? `${line.slice(0, 15)}…` : line)));
    expect(wrapWords("a b c d e f", 3, 2)).toEqual(["a b", "c…"]);
  });

  it("replaces_only_the_theme_members_and_keeps_name_logo_pages_and_drops_status", () => {
    const next = withLook(ORGANIZATION, { font: "serif" }) as { spec: { appearance: Record<string, unknown> }; status?: unknown };
    expect(next.status).toBeUndefined();
    expect(next.spec.appearance).toEqual({
      instanceName: "BBSK",
      logoSvg: ORGANIZATION.spec.appearance.logoSvg,
      pages: ORGANIZATION.spec.appearance.pages,
      font: "serif",
    });
  });
});

describe("open data appearance", () => {
  it("publishes_the_look_as_a_change_after_drawing_it_at_1440_and_400", async () => {
    const { container, sent } = renderAppearance();
    const user = userEvent.setup();
    await screen.findByTestId("opendata-appearance-editor");
    // Every field carries a hint with an example.
    expect(screen.getByText(/e\.g\. "Open data of Banská Bystrica"/)).toBeInTheDocument();
    expect(screen.getByText(/Radius of cards|Corner radius of cards/)).toBeInTheDocument();

    await user.type(screen.getByRole("textbox", { name: /Heading/ }), "Otvorené údaje BBSK");
    fireEvent.change(screen.getByRole("textbox", { name: "Introduction" }), { target: { value: "Údaje **mesta**" } });
    fireEvent.change(screen.getByLabelText("Typeface"), { target: { value: "serif" } });
    fireEvent.change(screen.getByLabelText("Dataset list"), { target: { value: "table" } });
    await user.click(screen.getAllByRole("button", { name: "Add link" })[0]);
    const header = screen.getByRole("group", { name: "Header links" });
    await user.type(within(header).getByRole("textbox", { name: "Label" }), "Mesto BB");
    await user.type(within(header).getByRole("textbox", { name: "Address" }), "https://www.banskabystrica.sk/");

    // Both widths are drawn from the form, before anything is saved; the intro comes back as elements.
    for (const width of [1440, 400]) {
      await user.click(screen.getByRole("tab", { name: `${width} px wide` }));
      const frame = screen.getByTestId(`preview-${width}`);
      expect(frame.textContent).toContain("Otvorené údaje BBSK");
      expect(frame.textContent).toContain("Mesto BB");
      expect(frame.getAttribute("font-family")).toContain("Georgia");
    }
    await waitFor(() => expect(screen.getAllByText("mesta")[0].tagName).toBe("STRONG"));
    expect(container.querySelector("script")).toBeNull();
    expect((window as { ran?: boolean }).ran).toBeUndefined();
    await user.click(screen.getByRole("tab", { name: "1440 px wide" }));
    expect(screen.getByTestId("preview-1440").textContent).toContain("Publisher");
    expect(sent).toEqual([]);

    await user.click(screen.getByRole("button", { name: "Publish appearance" }));
    await screen.findByText(/shows the new look within seconds/);
    // A checked proposal: the dry run, then the proposal, of the Organization manifest.
    expect(sent.map((item) => [item.path, item.dryRun])).toEqual([
      ["/api/v1/projects/org/organizations/bbsk", true],
      ["/api/v1/projects/org/organizations/bbsk", false],
    ]);
    const appearance = (proposals(sent)[0].body as { spec: { appearance: Record<string, unknown> } }).spec.appearance;
    expect(appearance).toEqual({
      instanceName: "BBSK",
      logoSvg: ORGANIZATION.spec.appearance.logoSvg,
      pages: ORGANIZATION.spec.appearance.pages,
      colours: { primary: "#0b5394", secondary: "#0f766e" },
      font: "serif",
      hero: { title: "Otvorené údaje BBSK", intro: "Údaje **mesta**" },
      datasetList: "table",
      navLinks: [{ label: "Mesto BB", url: "https://www.banskabystrica.sk/" }],
    });
    expect((proposals(sent)[0].body as { status?: unknown }).status).toBeUndefined();
  });

  it("shows_contrast_live_and_refuses_a_pair_below_aa_before_sending", async () => {
    const { sent } = renderAppearance();
    await screen.findByTestId("opendata-appearance-editor");
    expect(screen.getByTestId("contrast-primary").textContent).toMatch(/readable/);
    fireEvent.change(screen.getByRole("textbox", { name: /^Background/ }), { target: { value: "#ffffff" } });
    fireEvent.change(screen.getByRole("textbox", { name: /^Text/ }), { target: { value: "#999999" } });
    expect(screen.getByTestId("contrast-text").textContent).toBe("Contrast 2.85:1, below 4.5:1");
    expect(screen.getByText(/Drawn with the refused colour/)).toBeInTheDocument();
    expect(screen.getByText(/Contrast 2\.85:1 with the background is below 4\.5:1/)).toBeInTheDocument();
    const publish = screen.getByRole("button", { name: "Publish appearance" });
    expect(publish).toBeDisabled();
    fireEvent.change(screen.getByRole("textbox", { name: /^Text/ }), { target: { value: "#1f2937" } });
    expect(publish).toBeEnabled();
    expect(sent).toEqual([]);
  });

  it("refuses_a_javascript_link_under_its_field", async () => {
    renderAppearance();
    const user = userEvent.setup();
    await screen.findByTestId("opendata-appearance-editor");
    await user.click(screen.getAllByRole("button", { name: "Add link" })[1]);
    const footer = screen.getByRole("group", { name: "Footer links" });
    await user.type(within(footer).getByRole("textbox", { name: "Label" }), "Zlý");
    await user.type(within(footer).getByRole("textbox", { name: "Address" }), "javascript:alert(1)");
    expect(within(footer).getByRole("textbox", { name: "Address" })).toHaveAccessibleDescription(/other schemes and \/\/host are refused/);
    expect(screen.getByRole("button", { name: "Publish appearance" })).toBeDisabled();
    // The summary above the buttons names the field and leads to it.
    expect(screen.getByRole("link", { name: "Footer links 1: Address" })).toHaveAttribute("href", "#footerLinks-0-url");
  });

  it("offers_only_this_organizations_portals_and_proposes_the_portal_override", async () => {
    const { sent } = renderAppearance();
    const user = userEvent.setup();
    await screen.findByTestId("opendata-appearance-editor");
    const target = screen.getByLabelText("Portal to style");
    await waitFor(() => expect(within(target).getAllByRole("option")).toHaveLength(2));
    expect(within(target).queryByRole("option", { name: /praha-data/ })).toBeNull();
    fireEvent.change(target, { target: { value: "bbsk-mobility" } });
    // The portal's own primary, and the Organization's members shown as "Organization's".
    expect(await screen.findByDisplayValue("#7f1d1d")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: /^Background/ })).toHaveAttribute("placeholder", "Organization's");
    fireEvent.change(screen.getByLabelText(/Corner radius/), { target: { value: "12" } });
    await user.click(screen.getByRole("button", { name: "Publish appearance" }));
    await screen.findByText(/shows the new look within seconds/);
    const body = proposals(sent)[0];
    expect(body.path).toBe("/api/v1/projects/org/portals/bbsk-mobility");
    expect((body.body as { spec: Record<string, unknown> }).spec).toEqual({
      organizationRef: "bbsk",
      host: "bbsk-mobility.example.sk",
      appearance: { colours: { primary: "#7f1d1d" }, radius: 12 },
    });
  });

  it("goes_back_to_the_look_under_it_after_a_confirmation", async () => {
    const { sent } = renderAppearance();
    const user = userEvent.setup();
    await screen.findByTestId("opendata-appearance-editor");
    await user.click(screen.getByRole("button", { name: "Reset to the built-in look" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(sent).toEqual([]);
    await user.click(within(dialog).getByRole("button", { name: "Reset to the built-in look" }));
    await screen.findByText(/shows the new look within seconds/);
    expect((proposals(sent)[0].body as { spec: { appearance: Record<string, unknown> } }).spec.appearance).toEqual({
      instanceName: "BBSK",
      logoSvg: ORGANIZATION.spec.appearance.logoSvg,
      pages: ORGANIZATION.spec.appearance.pages,
    });
  });

  it("says_who_may_when_the_server_refuses_the_proposal", async () => {
    renderAppearance({
      write: new Response(JSON.stringify({ type: "about:blank", title: "Forbidden", status: 403, detail: "propose Organization is not granted" }), {
        status: 403,
        headers: { "content-type": "application/problem+json" },
      }),
    });
    const user = userEvent.setup();
    await screen.findByTestId("opendata-appearance-editor");
    await user.click(screen.getByRole("button", { name: "Publish appearance" }));
    expect(await screen.findByText(/only the Organization.s administrators can\. Ask one of them\. Server: propose Organization is not granted/)).toBeInTheDocument();
    // A second try ends the same way: Publish says why it is off.
    expect(screen.getByRole("button", { name: "Publish appearance" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Publish appearance" })).toHaveAccessibleDescription(/only the Organization.s administrators/);
  });

  it("passes_axe", async () => {
    const { container } = renderAppearance({}, true);
    await screen.findByTestId("opendata-appearance-editor");
    await expectNoAxeViolations(container);
  });
});
