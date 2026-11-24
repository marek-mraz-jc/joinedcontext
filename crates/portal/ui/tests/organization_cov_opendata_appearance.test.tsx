/**
 * Administration → Open data → Appearance: Retry after a failed read, the appearance history,
 * an open Change that holds Publish back, the summary of refusals, removing a link and the
 * dataset page of the preview.
 */
import { cleanup, fireEvent, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { OpendataAppearance } from "../src/pages/organization/OpendataAppearance";
import { json, problem, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding", "Organization", "Portal"], verbs: ["read", "propose", "approve", "delete"] } }],
} as unknown as Effective;

const ORGANIZATION = {
  apiVersion: "joinedcontext.com/v1",
  kind: "Organization",
  metadata: { name: "bbsk" },
  spec: { domain: "bbsk.sk", appearance: { instanceName: "BBSK" } },
  status: { phase: "Ready" },
};

const list = (items: unknown[]) => ({ apiVersion: "v1", kind: "List", items });

const change = (name: string, phase: string, params: Record<string, unknown> = { kind: "Organization", name: "bbsk" }) => ({
  apiVersion: "joinedcontext.com/v1",
  kind: "ChangeProposal",
  metadata: { name, namespace: "org" },
  author: { name: "Jana Admin", email: "jana@bbsk.sk" },
  createdAt: "2026-11-15T10:00:00Z",
  decision: phase === "Merged" ? { at: "2026-11-15T11:00:00Z", by: "eva" } : null,
  status: { phase, lane: "yellow", plan: {} },
  summary: { key: "update", params },
});

interface Setup {
  organizations?: () => Response;
  open?: unknown[];
  history?: unknown[];
  fields?: Record<string, unknown[]>;
}

function renderAppearance(setup: Setup = {}) {
  const reads: string[] = [];
  renderPage(<OpendataAppearance />, {
    path: "/organization/opendata-appearance",
    answer: (url) => {
      if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
      if (url.pathname === "/api/v1/projects/org/organizations") {
        reads.push(url.pathname);
        return setup.organizations ? setup.organizations() : json(list([ORGANIZATION]));
      }
      if (url.pathname === "/api/v1/projects/org/portals") return json(list([]));
      if (url.pathname === "/api/v1/organization/opendata/pages/preview") return json({ title: null, html: "<p>x</p>" });
      if (url.pathname === "/api/v1/projects/org/changes") return json(list(setup.open ?? []));
      if (url.pathname === "/api/v1/projects/org/changes/history") return json(list(setup.history ?? []));
      const detail = /\/api\/v1\/projects\/org\/changes\/(chg-[\w]+)$/.exec(url.pathname);
      if (detail) {
        const fields = setup.fields?.[detail[1]] ?? [];
        return json({ ...change(detail[1], "Merged"), planFields: fields });
      }
      return undefined;
    },
  });
  return reads;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(cleanup);

describe("open data appearance: a failed read", () => {
  it("says why the Organization could not be read and shows the editor after Retry", async () => {
    let healthy = false;
    const reads = renderAppearance({
      organizations: () => (healthy ? json(list([ORGANIZATION])) : problem(503, "The Organization store is restarting.")),
    });
    expect(await screen.findByText(/The Organization store is restarting/)).toBeInTheDocument();
    healthy = true;
    await userEvent.setup().click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByTestId("opendata-appearance-editor")).toBeInTheDocument();
    expect(reads).toHaveLength(2);
  });
});

describe("open data appearance: history", () => {
  it("lists only the closed Changes that wrote a theme member", async () => {
    renderAppearance({
      history: [change("chg-colour01", "Merged"), change("chg-whole002", "Merged"), change("chg-pages003", "Merged"), change("chg-other004", "Merged")],
      fields: {
        "chg-colour01": [{ path: "spec.appearance.colours.primary" }],
        // The whole appearance replaced: it counts when either side names a theme member.
        "chg-whole002": [{ path: "spec.appearance", from: { radius: 2 }, to: { instanceName: "BBSK" } }],
        "chg-pages003": [{ path: "spec.appearance", from: { pages: [] }, to: { pages: [{ page: "connector" }] } }],
        "chg-other004": [{ path: "spec.contacts" }],
      },
    });
    const history = await screen.findByTestId("opendata-pages-history");
    const hrefs = within(history).getAllByRole("link").map((link) => link.getAttribute("href") ?? "");
    expect(hrefs).toHaveLength(2);
    expect(hrefs[0]).toContain("chg-colour01");
    expect(hrefs[1]).toContain("chg-whole002");
  });

  it("says no appearance change was decided yet when none wrote a theme member", async () => {
    renderAppearance({ history: [change("chg-other004", "Merged")], fields: { "chg-other004": [{ path: "spec.contacts" }] } });
    expect(await screen.findByText(en.opendataAppearance.historyEmpty)).toBeInTheDocument();
  });
});

describe("open data appearance: the editor", () => {
  it("holds Publish back while a Change of this Organization waits, and links to it", async () => {
    renderAppearance({
      open: [change("chg-open0001", "PendingApproval"), change("chg-other999", "PendingApproval", { kind: "Organization", name: "praha" }), change("chg-merged00", "Merged")],
    });
    const warning = await screen.findByText(en.opendataAppearance.openChange, { exact: false });
    const links = within(warning.closest("[id=appearance-blocked]") as HTMLElement).getAllByRole("link");
    expect(links.map((link) => link.textContent)).toEqual(["chg-open0001"]);
    expect(links[0].getAttribute("href")).toContain("/approvals/chg-open0001");
    expect(screen.getByRole("button", { name: en.opendataAppearance.publish })).toBeDisabled();
  });

  it("names the radius and the intro in the summary of what is refused", async () => {
    renderAppearance();
    await screen.findByTestId("opendata-appearance-editor");
    fireEvent.change(screen.getByLabelText(/Corner radius/), { target: { value: "99" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Introduction" }), { target: { value: "x".repeat(2049) } });
    const radius = screen.getByRole("link", { name: en.organization.field.appearance.radius });
    expect(radius).toHaveAttribute("href", "#appearance-radius");
    expect(screen.getByRole("link", { name: en.organization.field.appearance.heroIntro })).toHaveAttribute("href", "#appearance-hero-intro");
    expect(screen.getByRole("button", { name: en.opendataAppearance.publish })).toBeDisabled();
  });

  it("removes a link by its label and leaves the others", async () => {
    renderAppearance();
    const user = userEvent.setup();
    await screen.findByTestId("opendata-appearance-editor");
    const header = screen.getByRole("group", { name: "Header links" });
    await user.click(within(header).getByRole("button", { name: en.opendataAppearance.addLink }));
    await user.click(within(header).getByRole("button", { name: en.opendataAppearance.addLink }));
    const labels = within(header).getAllByRole("textbox", { name: "Label" });
    await user.type(labels[0], "Mesto");
    await user.type(labels[1], "Kraj");
    await user.click(within(header).getByRole("button", { name: "Remove link Mesto" }));
    expect(within(header).getAllByRole("textbox", { name: "Label" })).toHaveLength(1);
    expect(within(header).getByRole("textbox", { name: "Label" })).toHaveValue("Kraj");
    // A link with no label is named by its number.
    await user.clear(within(header).getByRole("textbox", { name: "Label" }));
    await user.click(within(header).getByRole("button", { name: "Remove link 1" }));
    expect(within(header).queryByRole("textbox", { name: "Label" })).toBeNull();
    expect(within(header).getByText(en.opendataAppearance.linksEmpty)).toBeInTheDocument();
  });

  it("draws the dataset page of the preview with the badge and the download button", async () => {
    renderAppearance();
    const user = userEvent.setup();
    await screen.findByTestId("opendata-appearance-editor");
    fireEvent.change(screen.getByRole("textbox", { name: /^Primary/ }), { target: { value: "#0b5394" } });
    fireEvent.change(screen.getByRole("textbox", { name: /^Accent/ }), { target: { value: "#ffe977" } });
    await user.click(screen.getByRole("tab", { name: en.opendataAppearance.previewDataset }));
    const frame = screen.getByTestId("preview-1440");
    expect(frame.textContent).toContain("CSV");
    expect(frame.textContent).toContain(en.opendataAppearance.download);
    expect(frame.textContent).toContain(en.opendataAppearance.exampleDistribution);
    // The text on a dark primary is white and on a light accent black, as the catalogue picks it.
    const fills = [...frame.querySelectorAll("text")].filter((node) => ["CSV", en.opendataAppearance.download].includes(node.textContent ?? ""));
    expect(fills.map((node) => node.getAttribute("fill"))).toEqual(["black", "white"]);
  });
});
