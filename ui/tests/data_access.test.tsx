/**
 * T-6412 (ADR-N-067 §2.5): Organization → Data access. The matrix names the Policy rule behind
 * each cell with a link that proposes a change of that Policy; the group, Endpoint and person
 * panels read the same answer narrowed. Each panel has its loading, empty, refusal (403) and
 * failure states.
 */
import { cleanup, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import {
  DataAccessMatrix,
  EndpointReaders,
  GroupDatasets,
  PersonReadable,
} from "../src/pages/organization/DataAccess";
import { OrganizationPage } from "../src/pages/organization/OrganizationPage";
import { MemberName } from "../src/pages/access/GroupPage";
import { AskAdmins } from "../src/pages/organization/AskAdmins";
import { PersonPage } from "../src/pages/organization/People";
import { DeniedNote } from "../src/components/ui/PermissionGuard";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding"], verbs: ["propose", "approve", "delete"] } }],
} as unknown as Effective;

const columns = [{ kind: "organization" }, { kind: "group", group: "traffic-analysts" }];
const cell = (rules: { policy: string; rule: number; types: string[] }[], throughOrganization = false) => ({
  granted: rules.length > 0 || throughOrganization,
  rules,
  throughOrganization,
  unanswered: false,
});
const row = (name: string, cells: ReturnType<typeof cell>[]) => ({
  project: "helsinki",
  name,
  slug: `${name}zzzzzzzzzzzzzzzzzzzzzz`,
  title: name === "counts" ? "Traffic counts" : null,
  audience: "organization",
  organization: "helsinki",
  cells,
});
const MATRIX = {
  columns,
  rows: [
    row("counts", [cell([{ policy: "staff-traffic", rule: 1, types: ["TrafficFlowObserved"] }]), cell([], true)]),
    row("vehicles", [cell([]), cell([{ policy: "staff-traffic", rule: 2, types: ["Vehicle"] }])]),
  ],
  unanswered: 0,
};

type Answer = Response | Promise<Response>;

function render(node: React.ReactElement, answer: (path: string, search: URLSearchParams) => Answer | undefined) {
  const requests: string[] = [];
  const result = renderPage(node, {
    path: "/organization/data-access",
    answer: (url) => {
      requests.push(`${url.pathname}${url.search}`);
      if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
      return answer(url.pathname, url.searchParams);
    },
  });
  return { ...result, requests };
}

const forbidden = () =>
  new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "read on Person" }), {
    status: 403,
    headers: { "content-type": "application/problem+json" },
  });
const broken = () =>
  new Response(JSON.stringify({ title: "Unavailable", status: 503, detail: "the gateway did not answer" }), {
    status: 503,
    headers: { "content-type": "application/problem+json" },
  });
const never = () => new Promise<Response>(() => undefined);

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
});

describe("Organization → Data access", () => {
  it("shows the matrix with the rule behind each cell and a link to propose a change", async () => {
    const { container } = render(<OrganizationPage tab="data-access" anchor="helsinki" />, (path) =>
      path === "/api/v1/organization/data-access" ? json(MATRIX) : undefined,
    );
    const table = await screen.findByRole("table", { name: en.dataAccess.title });
    expect(within(table).getByText(en.dataAccess.everyone)).toBeInTheDocument();
    expect(within(table).getByText("Group traffic-analysts")).toBeInTheDocument();
    const counts = within(table).getByText("Traffic counts").closest("tr") as HTMLElement;
    const link = within(counts).getByRole("link", { name: "Propose a change of staff-traffic, rule 1" });
    expect(link).toHaveAttribute("href", "/projects/helsinki/policies/staff-traffic/edit");
    expect(within(counts).getByText("TrafficFlowObserved")).toBeInTheDocument();
    // The analysts read counts through everyone, so their cell names no rule of its own.
    expect(within(counts).getByText(en.dataAccess.throughEveryone)).toBeInTheDocument();
    const vehicles = within(table).getByText("vehicles").closest("tr") as HTMLElement;
    expect(within(vehicles).getAllByText(en.dataAccess.notGranted)).toHaveLength(1);
    // The legend under the table says what the pill and the dash mean.
    expect(screen.getByText(en.dataAccess.legend.throughEveryone)).toBeInTheDocument();
    expect(screen.getByText(en.dataAccess.legend.notGranted)).toBeInTheDocument();
    await expectNoAxeViolations(container);
  });

  it("says what to do when every Endpoint is public", async () => {
    render(<DataAccessMatrix />, () => json({ columns: columns.slice(0, 1), rows: [], unanswered: 0 }));
    expect(await screen.findByText(en.dataAccess.empty.title)).toBeInTheDocument();
  });

  it("says whose page it is on a 403, a failure with a retry otherwise, and loads first", async () => {
    render(<DataAccessMatrix />, forbidden);
    expect(await screen.findByText(en.dataAccess.adminOnly)).toBeInTheDocument();
    cleanup();
    render(<DataAccessMatrix />, broken);
    expect(await screen.findByRole("button", { name: /try again|retry/i })).toBeInTheDocument();
    cleanup();
    render(<DataAccessMatrix />, never);
    // Said once to a screen reader, and in a visible line beside the skeleton.
    expect(await screen.findByRole("status")).toHaveTextContent(en.dataAccess.loading);
    expect(screen.getAllByText(en.dataAccess.loading)).toHaveLength(2);
  });

  it("fails with a retry, never a broken page, on an answer without rows", async () => {
    render(<DataAccessMatrix />, () => json({ items: [] }));
    expect(await screen.findByRole("button", { name: /try again|retry/i })).toBeInTheDocument();
  });

  it("warns about cells the gateway did not answer", async () => {
    render(<DataAccessMatrix />, () => json({ ...MATRIX, unanswered: 3 }));
    expect(await screen.findByText(/did not answer for 3 cells/)).toBeInTheDocument();
  });
});

describe("Group page: datasets this group reads", () => {
  it("asks for the group's column and lists what it reads", async () => {
    const { requests } = render(<GroupDatasets group="traffic-analysts" />, () => json(MATRIX));
    expect(await screen.findByText("vehicles")).toBeInTheDocument();
    // counts reaches the group through everyone: named below the group's own list, by title.
    const shared = screen.getByText(en.dataAccess.groupEveryone).nextElementSibling as HTMLElement;
    expect(within(shared).getByRole("link", { name: "Traffic counts" })).toBeInTheDocument();
    expect(within(shared).queryByText("vehicles")).toBeNull();
    expect(requests).toContain("/api/v1/organization/data-access?group=traffic-analysts");
  });

  it("is empty with a next step, and refused for a non-admin", async () => {
    render(<GroupDatasets group="planners" />, () =>
      json({ columns: [columns[0], { kind: "group", group: "planners" }], rows: [], unanswered: 0 }),
    );
    expect(await screen.findByText("planners reads no internal dataset")).toBeInTheDocument();
    cleanup();
    render(<GroupDatasets group="planners" />, forbidden);
    expect(await screen.findByText(en.dataAccess.adminOnly)).toBeInTheDocument();
  });
});

describe("Endpoint page: who can read this", () => {
  it("names the audience and each grant in plain words", async () => {
    const one = { columns, rows: [MATRIX.rows[0]], unanswered: 0 };
    const { requests } = render(<EndpointReaders slug="countszzzzzzzzzzzzzzzzzz" audience="organization" />, () => json(one));
    expect(await screen.findByText(/Only signed-in members of helsinki reach it/)).toBeInTheDocument();
    expect(screen.getByText(en.dataAccess.endpointEveryone)).toBeInTheDocument();
    // The analysts read it through everyone: no second line for them.
    expect(screen.queryByText(/traffic-analysts read it/)).toBeNull();
    expect(requests).toContain("/api/v1/organization/data-access?endpoint=countszzzzzzzzzzzzzzzzzz");
  });

  it("names a reading group as a link to its page", async () => {
    const granted = { columns, rows: [row("vehicles", [cell([]), cell([{ policy: "staff-traffic", rule: 2, types: ["Vehicle"] }])])], unanswered: 0 };
    render(<EndpointReaders slug="vehizzzzzzzzzzzzzzzzzzzz" audience="organization" />, () => json(granted));
    expect(await screen.findByRole("link", { name: "traffic-analysts" })).toHaveAttribute("href", "/organization/groups/traffic-analysts");
  });

  it("says a public Endpoint is open without asking, and nobody when no grant names a group", async () => {
    const { requests } = render(<EndpointReaders slug="bikeszzzzzzzzzzzzzzzzzzz" audience="public" />, () => undefined);
    expect(await screen.findByText(en.dataAccess.endpointPublic)).toBeInTheDocument();
    expect(requests.filter((r) => r.includes("data-access"))).toEqual([]);
    cleanup();
    const closed = { columns, rows: [row("closed", [cell([]), cell([])])], unanswered: 0 };
    render(<EndpointReaders slug="closedzzzzzzzzzzzzzzzzzz" audience="organization" />, () => json(closed));
    expect(await screen.findByText(en.dataAccess.endpointNobody)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.dataAccess.endpointNext })).toHaveAttribute("href", "/projects/helsinki/policies");
    cleanup();
    render(<EndpointReaders slug="closedzzzzzzzzzzzzzzzzzz" audience="organization" />, forbidden);
    expect(await screen.findByText(en.dataAccess.adminOnly)).toBeInTheDocument();
  });
});

describe("Person page: what this person can read", () => {
  const dataset = (name: string, access: string, group?: string) => ({
    project: "helsinki",
    name,
    slug: `${name}zzzzzzzzzzzzzzzzzzzzzz`,
    title: null,
    description: null,
    representations: [],
    access,
    group,
  });

  it("lists the internal datasets by name with what grants each, and counts the public ones", async () => {
    const answer = { datasets: [dataset("bikes", "public"), dataset("vehicles", "group", "traffic-analysts"), dataset("counts", "organization")], unanswered: 0 };
    const { container, requests } = render(<PersonReadable id="jana-id" name="Jana" />, () => json(answer));
    expect(await screen.findByText("vehicles")).toBeInTheDocument();
    expect(screen.queryByText("bikes")).toBeNull();
    // Worded about Jana, not the administrator reading the panel.
    expect(screen.getByText(en.dataAccess.personAccess.organization)).toBeInTheDocument();
    expect(screen.queryByText(en.catalogue.readable.access.organization)).toBeNull();
    expect(screen.getByText("Public datasets anyone can read, not listed here: 1.")).toBeInTheDocument();
    expect(requests).toContain("/api/v1/organization/people/jana-id/readable");
    // Names only: nothing on the panel opens the dataset as the administrator.
    expect(within(container).queryAllByRole("link")).toHaveLength(0);
  });

  it("is empty with a next step, and refused for a non-admin", async () => {
    render(<PersonReadable id="jana-id" name="Jana" />, () => json({ datasets: [], unanswered: 0 }));
    expect(await screen.findByText("Jana reads no internal dataset")).toBeInTheDocument();
    cleanup();
    render(<PersonReadable id="jana-id" name="Jana" />, forbidden);
    expect(await screen.findByText(en.dataAccess.adminOnly)).toBeInTheDocument();
  });
});

describe("T-6423: names, not ids, and a refusal that says whom to ask", () => {
  const ORGANIZATION = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Organization",
    metadata: { name: "helsinki", namespace: "org" },
    spec: { installation: true, contacts: [{ role: "administrative", name: "Ada Admin", email: "ada@example.org" }] },
  };
  const READER = { project: "org", bootstrap: false, grants: [{ role: "r", binding: "b", rule: { kinds: ["*"], verbs: ["read"] } }] };

  it("names the organization's contacts on a refused panel", async () => {
    render(<GroupDatasets group="planners" />, (path) =>
      path === "/api/v1/projects/org/organizations" ? json({ kind: "List", items: [ORGANIZATION] }) : forbidden(),
    );
    expect(await screen.findByText(en.askAdmins.title)).toBeInTheDocument();
    expect(await screen.findByText(/Ada Admin, ada@example\.org/)).toBeInTheDocument();
    expect(screen.getByText(en.dataAccess.adminOnly)).toBeInTheDocument();
    // A refused panel keeps its page: no way back is offered inside it.
    expect(screen.queryByRole("link", { name: en.askAdmins.back })).toBeNull();
  });

  it("refuses the page with the way back to the person's projects", async () => {
    renderPage(<OrganizationPage tab="data-access" anchor="helsinki" />, {
      path: "/organization/data-access",
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return json(READER);
        if (url.pathname === "/api/v1/projects/org/organizations") return json({ kind: "List", items: [] });
        return undefined;
      },
    });
    expect(await screen.findByText(en.organization.adminOnly)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.askAdmins.back })).toHaveAttribute("href", "/projects/helsinki/home");
  });

  it("names a group member by name with a link to their page, a plain label over the folded id when no name is known", async () => {
    const id = "3f2a9c1e-7b4d-4e8a-9f61-2c5d8e0b7a14";
    renderPage(<MemberName id={id} names={new Map([[id, "Jana Nováková"]])} />, { path: "/", answer: () => undefined });
    const link = await screen.findByRole("link", { name: "Jana Nováková" });
    expect(link).toHaveAttribute("href", `/organization/people/${id}`);
    expect(screen.queryByText(id)).toBeNull();
    cleanup();
    renderPage(<MemberName id={id} names={new Map()} />, { path: "/", answer: () => undefined });
    const label = await screen.findByText(en.access.groupPage.hiddenName);
    expect(label.closest("details")).not.toHaveAttribute("open");
    expect(label.closest("details")).toHaveTextContent(id);
  });

  it("answers a refused person page with whom to ask, the id nowhere", async () => {
    const id = "3f2a9c1e-7b4d-4e8a-9f61-2c5d8e0b7a14";
    renderPage(<PersonPage id={id} anchor="helsinki" />, { path: "/", answer: () => forbidden() });
    expect(await screen.findByText(en.organization.adminOnly)).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: en.organization.people.title })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: en.askAdmins.back })).toHaveAttribute("href", "/projects/helsinki/home");
    expect(screen.queryByText(id)).toBeNull();
  });

  it("says the sentence alone when the Organization cannot be read", async () => {
    renderPage(<AskAdmins message="Ask them." back="helsinki" />, { path: "/", answer: () => forbidden() });
    expect(await screen.findByText("Ask them.")).toBeInTheDocument();
    expect(screen.queryByText(en.askAdmins.contacts)).toBeNull();
    expect(screen.getByRole("link", { name: en.askAdmins.back })).toBeInTheDocument();
  });

  it("says in a visible line why a reader's controls are off", async () => {
    renderPage(<DeniedNote project="org" kind="Group" verb="propose" />, {
      path: "/",
      answer: (url) => (url.pathname.endsWith("/permissions/me") ? json(READER) : undefined),
    });
    expect(await screen.findByText("Only an organization administrator can add or change groups.")).toBeInTheDocument();
  });
});
