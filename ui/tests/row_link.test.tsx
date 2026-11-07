/**
 * T-2875: every record opens on click. A click anywhere on a row whose record has a link
 * (`RecordLink`, or a list's own link marked `data-row-link`) opens that link; a click on what
 * handles its own click (a button, a menu, another link, a portalled dialog) does not; Ctrl, Cmd,
 * Shift and the middle button open a new tab. Then each list of the Portal, mounted through its
 * route: a click on a plain cell of a row lands on that record's page or edit form, and a click
 * on the row's own action button leaves the list where it is.
 */
import { useState } from "react";
import { createPortal } from "react-dom";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from "@tanstack/react-router";
import { Table, TableBody, TableCell, TableRow } from "../src/components/ui/Table";
import { LocalRecordLink, RecordLink } from "../src/components/RecordLink";
import { jsonResponse, list, renderRoute } from "./pageHarness";
import { pageFindings } from "./pageChecks";

// Monaco cannot run in jsdom: the read-only YAML view stands in as a labelled, read-only text box.
vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: (props: { value: string; ariaLabel?: string; readOnly?: boolean }) => (
    <textarea aria-label={props.ariaLabel} readOnly={Boolean(props.readOnly)} value={props.value} />
  ),
}));

function Portalled(): React.JSX.Element {
  return createPortal(<span>in a dialog</span>, document.body);
}

function Row({ opened, inner, linked = true }: { opened: () => void; inner: () => void; linked?: boolean }) {
  const [selected, setSelected] = useState(false);
  return (
    <Table caption="Records">
      <TableBody>
        <TableRow>
          <TableCell>
            {linked ? (
              <a
                data-row-link=""
                href="/projects/helsinki/pipelines/ingest/edit"
                onClick={(event) => {
                  event.preventDefault();
                  opened();
                }}
              >
                ingest
              </a>
            ) : (
              "ingest"
            )}
          </TableCell>
          <TableCell>Live</TableCell>
          <TableCell>
            <button type="button" onClick={inner}>
              Pause
            </button>
            <input
              type="checkbox"
              aria-label="Pick ingest"
              checked={selected}
              onChange={() => {
                setSelected(!selected);
              }}
            />
          </TableCell>
          <TableCell>
            <Portalled />
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  );
}

describe("a table row with a record link", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("opens the link on a click anywhere else in the row", async () => {
    const opened = vi.fn();
    render(<Row opened={opened} inner={vi.fn()} />);
    await userEvent.click(screen.getByText("Live"));
    expect(opened).toHaveBeenCalledTimes(1);
  });

  it("leaves a button, a checkbox and the link itself to their own click", async () => {
    const opened = vi.fn();
    const inner = vi.fn();
    render(<Row opened={opened} inner={inner} />);
    await userEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(inner).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("checkbox", { name: "Pick ingest" }));
    expect(screen.getByRole("checkbox", { name: "Pick ingest" })).toBeChecked();
    expect(opened).not.toHaveBeenCalled();
    // The link opens once, by itself, not a second time through the row.
    await userEvent.click(screen.getByRole("link", { name: "ingest" }));
    expect(opened).toHaveBeenCalledTimes(1);
  });

  it("ignores a click inside what the row renders through a portal", async () => {
    const opened = vi.fn();
    render(<Row opened={opened} inner={vi.fn()} />);
    await userEvent.click(screen.getByText("in a dialog"));
    expect(opened).not.toHaveBeenCalled();
  });

  it("hands a Ctrl click and a middle click to the link with the keys of a new tab", async () => {
    const keys: string[] = [];
    render(
      <Table caption="Records">
        <TableBody>
          <TableRow>
            <TableCell>
              <a
                data-row-link=""
                href="/projects/helsinki/pipelines/ingest/edit"
                onClick={(event) => {
                  event.preventDefault();
                  keys.push(`${event.ctrlKey ? "ctrl" : ""}${event.metaKey ? "+meta" : ""}`);
                }}
              >
                ingest
              </a>
            </TableCell>
            <TableCell>Live</TableCell>
          </TableRow>
        </TableBody>
      </Table>,
    );
    const user = userEvent.setup();
    await user.keyboard("{Control>}");
    await user.click(screen.getByText("Live"));
    await user.keyboard("{/Control}");
    fireEvent(screen.getByText("Live"), new MouseEvent("auxclick", { bubbles: true, button: 1 }));
    expect(keys).toEqual(["ctrl", "ctrl+meta"]);
  });

  it("does not open when the click ends a text selection", async () => {
    const opened = vi.fn();
    vi.spyOn(window, "getSelection").mockReturnValue({ toString: () => "Live" } as Selection);
    render(<Row opened={opened} inner={vi.fn()} />);
    await userEvent.click(screen.getByText("Live"));
    expect(opened).not.toHaveBeenCalled();
  });

  it("is an ordinary row when the record has no link", async () => {
    const before = window.location.pathname;
    render(<Row opened={vi.fn()} inner={vi.fn()} linked={false} />);
    await userEvent.click(screen.getByText("Live"));
    expect(window.location.pathname).toBe(before);
  });
});

describe("a record link", () => {
  it("opens a record's own page where it has one, else its edit form, and marks its row", async () => {
    const root = createRootRoute({
      component: () => (
        <>
          <RecordLink project="helsinki" plural="spaces" name="air" />
          <RecordLink project="helsinki" plural="pipelines" name="ingest">
            Ingest the feed
          </RecordLink>
          <RecordLink project="helsinki" plural="datasources" name="feed">
            The feed
          </RecordLink>
        </>
      ),
    });
    render(<RouterProvider router={createRouter({ routeTree: root, history: createMemoryHistory() })} />);
    expect(await screen.findByRole("link", { name: "air" })).toHaveAttribute("href", "/projects/helsinki/spaces/air");
    const pipeline = screen.getByRole("link", { name: "Ingest the feed" });
    // A pipeline has its own page since T-4817; a data source still opens on its form.
    expect(pipeline).toHaveAttribute("href", "/projects/helsinki/pipelines/ingest");
    expect(pipeline).toHaveAttribute("data-row-link");
    expect(screen.getByRole("link", { name: "The feed" })).toHaveAttribute(
      "href",
      "/projects/helsinki/datasources/feed/edit",
    );
  });

  it("opens in place on a plain click and leaves a modified click to the browser's new tab", () => {
    const onOpen = vi.fn();
    render(
      <LocalRecordLink href="?open=c-1" onOpen={onOpen}>
        The question
      </LocalRecordLink>,
    );
    const link = screen.getByRole("link", { name: "The question" });
    expect(link).toHaveAttribute("href", "?open=c-1");
    expect(link).toHaveAttribute("data-row-link");
    expect(fireEvent.click(link)).toBe(false);
    expect(onOpen).toHaveBeenCalledTimes(1);
    for (const keys of [{ ctrlKey: true }, { metaKey: true }, { shiftKey: true }, { altKey: true }, { button: 1 }]) {
      expect(fireEvent.click(link, keys)).toBe(true);
    }
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("refuses an address that is not safe to follow", () => {
    render(
      <LocalRecordLink href="javascript:alert(1)" onOpen={vi.fn()}>
        Unsafe
      </LocalRecordLink>,
    );
    expect(screen.getByText("Unsafe").closest("a")?.getAttribute("href") ?? "").not.toContain("javascript");
  });
});

const manifest = (kind: string, name: string, spec: Record<string, unknown> = {}) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace: "helsinki" },
  spec,
  status: { phase: "Live" },
});

interface Listed {
  list: string;
  path: string;
  /** The API path that answers the list's items, and the answer. */
  api: string;
  body: unknown;
  /** The row's link text. */
  record: string;
  opens: string;
  /** More reads the page makes, by API path. */
  more?: Record<string, unknown>;
  /** What the opened edit form shows, where the list hosts its forms at an address of its own. */
  shows?: string;
}

const LISTS: Listed[] = [
  {
    list: "CKAN catalogues",
    path: "/projects/helsinki/ckan",
    api: "/api/v1/projects/helsinki/ckan/status",
    body: {
      instances: [{ name: "open-data", url: "https://data.hel.fi", organizationDefault: "helsinki", apiTokenRef: "ckan-open-data" }],
      publications: [],
    },
    more: {
      "/api/v1/projects/helsinki/ckaninstances": list([manifest("CkanInstance", "open-data", { url: "https://data.hel.fi" })]),
      "/api/v1/projects/helsinki/ckaninstances/open-data": manifest("CkanInstance", "open-data", { url: "https://data.hel.fi" }),
    },
    record: "open-data",
    opens: "/projects/helsinki/ckaninstances/open-data/edit",
    shows: "Edit open-data",
  },
  {
    list: "project roles",
    path: "/projects/helsinki/settings/roles",
    api: "/api/v1/projects/helsinki/roles",
    body: list([manifest("Role", "editors", { rules: [{ kinds: ["Pipeline"], verbs: ["read"] }] })]),
    record: "editors",
    opens: "/projects/helsinki/settings/roles/editors/edit",
    shows: "Edit editors",
  },
  {
    list: "organization roles",
    path: "/organization/roles",
    api: "/api/v1/projects/org/roles",
    body: list([manifest("Role", "auditors", { rules: [{ kinds: ["Pipeline"], verbs: ["read"] }] })]),
    record: "auditors",
    opens: "/organization/roles/auditors/edit",
    shows: "Edit auditors",
  },
  {
    list: "project members",
    path: "/projects/helsinki/settings/members",
    api: "/api/v1/projects/org/rolebindings",
    body: list([
      manifest("RoleBinding", "jana-editors", {
        subjects: [{ user: "jana@hel.fi" }],
        role: "editors",
        scope: { project: "helsinki" },
      }),
    ]),
    more: { "/api/v1/projects/helsinki/spaces": list([]) },
    record: "jana@hel.fi",
    opens: "/projects/helsinki/settings/members/jana-editors/edit",
    shows: "Edit jana@hel.fi: editors",
  },
  {
    list: "pipelines",
    path: "/projects/helsinki/pipelines",
    api: "/api/v1/projects/helsinki/pipelines",
    body: list([manifest("Pipeline", "ingest", { class: "resident" })]),
    record: "ingest",
    opens: "/projects/helsinki/pipelines/ingest",
  },
  {
    list: "data sources",
    path: "/projects/helsinki/datasources",
    api: "/api/v1/projects/helsinki/datasources",
    body: list([manifest("DataSource", "feed", { type: "http", http: { url: "https://example.org/feed.json" } })]),
    record: "feed",
    opens: "/projects/helsinki/datasources/feed/edit",
  },
  {
    list: "subscriptions",
    path: "/projects/helsinki/subscriptions",
    api: "/api/v1/projects/helsinki/subscriptions",
    body: list([manifest("Subscription", "alerts", { contextSpaceRef: "air" })]),
    record: "alerts",
    opens: "/projects/helsinki/subscriptions/alerts/edit",
  },
  {
    list: "policies",
    path: "/projects/helsinki/policies",
    api: "/api/v1/projects/helsinki/policies",
    body: list([manifest("Policy", "readers", { contextSpaceRef: { kind: "ContextSpace", name: "air" } })]),
    record: "readers",
    opens: "/projects/helsinki/policies/readers/edit",
  },
  {
    list: "context source registrations",
    path: "/projects/helsinki/csrs",
    api: "/api/v1/projects/helsinki/csrs",
    body: list([manifest("ContextSourceRegistration", "weather", { contextSpaceRef: "air" })]),
    record: "weather",
    opens: "/projects/helsinki/csrs/weather",
  },
  {
    list: "context spaces",
    path: "/projects/helsinki/spaces",
    api: "/api/v1/projects/helsinki/spaces",
    body: list([manifest("ContextSpace", "air")]),
    record: "air",
    opens: "/projects/helsinki/spaces/air",
  },
  {
    list: "endpoints",
    path: "/projects/helsinki/endpoints",
    api: "/api/v1/projects/helsinki/endpoints",
    body: list([manifest("Endpoint", "public-air", { contextSpaceRef: "air", audience: "public" })]),
    record: "public-air",
    opens: "/projects/helsinki/endpoints/public-air",
  },
  {
    list: "a kind without a page of its own",
    path: "/projects/helsinki/blueprints",
    api: "/api/v1/projects/helsinki/blueprints",
    body: list([manifest("Blueprint", "alerting")]),
    record: "alerting",
    opens: "/projects/helsinki/blueprints/alerting/edit",
  },
  {
    list: "approvals",
    path: "/projects/helsinki/approvals",
    api: "/api/v1/projects/helsinki/changes",
    body: list([
      {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "ChangeProposal",
        metadata: { name: "chg-1a2b3c4d", namespace: "helsinki" },
        summary: { key: "change.summary.update", params: { kind: "Endpoint", name: "public-air", fields: 2 } },
        author: { name: "Marek Mráz", email: "marek@hel.fi" },
        createdAt: "2026-03-03T12:00:00Z",
        status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
      },
    ]),
    record: "",
    opens: "/projects/helsinki/approvals/chg-1a2b3c4d",
  },
  {
    list: "people",
    path: "/organization/people",
    api: "/api/v1/organization/people",
    body: {
      items: [
        {
          id: "jana-id",
          email: "jana@hel.fi",
          firstName: "Jana",
          lastName: "Nováková",
          enabled: true,
          emailVerified: true,
          requiredActions: [],
          createdAt: "2026-09-01T08:00:00Z",
          lastSeen: null,
          locale: "sk",
          pendingDeletion: null,
        },
      ],
    },
    record: "Jana Nováková",
    opens: "/organization/people/jana-id",
  },
];

describe("every list opens its record on a click on the row", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  for (const one of LISTS) {
    it(`${one.list}: a plain cell opens the record, the row's own button does not`, async () => {
      await renderRoute({
        path: `${one.path}?lang=en`,
        answer: (path, request) => {
          if (request.method !== "GET") return undefined;
          if (path === one.api) return jsonResponse(one.body);
          if (one.more && path in one.more) return jsonResponse(one.more[path]);
          return undefined;
        },
      });
      const link = await waitFor(() => {
        const found = document.querySelector<HTMLAnchorElement>("a[data-row-link]");
        expect(found, `${one.list} renders a row link`).not.toBeNull();
        return found as HTMLAnchorElement;
      });
      if (one.record) {
        expect(link).toHaveTextContent(one.record);
      }
      const row = link.closest("tr") as HTMLTableRowElement;
      const button = within(row).queryAllByRole("button")[0];
      if (button) {
        await userEvent.click(button);
        await userEvent.keyboard("{Escape}");
        expect(window.location.pathname, "the row's button handled its own click").toBe(one.path);
      }
      const plain = Array.from(row.querySelectorAll("td")).find(
        (cell) => cell.querySelector("a, button, input, select, textarea, [role=button]") === null,
      );
      expect(plain, `${one.list} has a cell with nothing of its own to click`).toBeDefined();
      await userEvent.click(plain as HTMLTableCellElement);
      await waitFor(() => {
        expect(window.location.pathname).toBe(one.opens);
      });
      if (one.shows) {
        expect((await screen.findAllByText(one.shows)).length).toBeGreaterThan(0);
        expect(screen.queryByText(/This form cannot be opened/)).toBeNull();
      }
    });
  }

  it("service accounts: the Edit button goes to the form's address and opens it", async () => {
    const account = manifest("ServiceAccount", "ingest-bot", {
      owner: { user: "jana@hel.fi" },
      purpose: "Feeds the air space",
      roles: [{ role: "editors", scope: { project: "helsinki" } }],
    });
    await renderRoute({
      path: "/projects/helsinki/settings/service-accounts?lang=en",
      answer: (path, request) => {
        if (request.method !== "GET") return undefined;
        if (path === "/api/v1/projects/helsinki/serviceaccounts") return jsonResponse(list([account]));
        return undefined;
      },
    });
    const link = await screen.findByRole("link", { name: "ingest-bot" });
    expect(link).toHaveAttribute("href", "/projects/helsinki/settings/service-accounts/ingest-bot/edit");
    await userEvent.click(within(link.closest("li") as HTMLElement).getByRole("button", { name: "Edit ingest-bot" }));
    await waitFor(() => {
      expect(window.location.pathname).toBe("/projects/helsinki/settings/service-accounts/ingest-bot/edit");
    });
    expect((await screen.findAllByText("Edit ingest-bot")).length).toBeGreaterThan(0);
  });

  it("service accounts: a plain click on the card goes to the form's address", async () => {
    const account = manifest("ServiceAccount", "ingest-bot", {
      owner: { user: "jana@hel.fi" },
      purpose: "Feeds the air space",
      roles: [],
    });
    await renderRoute({
      path: "/projects/helsinki/settings/service-accounts?lang=en",
      answer: (path, request) => {
        if (request.method !== "GET") return undefined;
        if (path === "/api/v1/projects/helsinki/serviceaccounts") return jsonResponse(list([account]));
        return undefined;
      },
    });
    await userEvent.click(await screen.findByText("Feeds the air space"));
    await waitFor(() => {
      expect(window.location.pathname).toBe("/projects/helsinki/settings/service-accounts/ingest-bot/edit");
    });
    expect((await screen.findAllByText("Edit ingest-bot")).length).toBeGreaterThan(0);
  });
});

const CARDS = [
  {
    list: "sync sources",
    path: "/projects/helsinki/syncsources",
    api: "/api/v1/projects/helsinki/syncsources",
    body: list([manifest("SyncSource", "city-repo", { source: { git: { url: "https://git.example.org/city.git" } } })]),
    link: "city-repo",
    plain: "Origin",
    opens: "/projects/helsinki/syncsources/city-repo/edit",
  },
  {
    list: "apps",
    path: "/projects/helsinki/apps",
    api: "/api/v1/projects/helsinki/apps",
    body: list([manifest("App", "bike-map", { lifecycle: "draft", visibility: "project" })]),
    link: /bike map/i,
    plain: "Visible to members of this project",
    opens: "/projects/helsinki/apps/bike-map",
  },
];

describe("every card of a record opens it on a click on the card", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  for (const one of CARDS) {
    it(`${one.list}: a plain part of the card opens the record, the card's own button does not`, async () => {
      await renderRoute({
        path: `${one.path}?lang=en`,
        answer: (path, request) => (request.method === "GET" && path === one.api ? jsonResponse(one.body) : undefined),
      });
      const link = await screen.findByRole("link", { name: one.link });
      expect(link).toHaveAttribute("data-row-link");
      expect(link).toHaveAttribute("href", one.opens);
      const card = link.closest("li") as HTMLElement;
      const button = within(card).queryAllByRole("button")[0];
      if (button) {
        await userEvent.click(button);
        await userEvent.keyboard("{Escape}");
        expect(window.location.pathname, "the card's button handled its own click").toBe(one.path);
      }
      await userEvent.click(within(card).getByText(one.plain));
      await waitFor(() => {
        expect(window.location.pathname).toBe(one.opens);
      });
    });
  }
});

describe("a copy of the project opens on a click on its row", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("workspaces: a plain cell opens the copy where its Open goes, Discard does not", async () => {
    await renderRoute({
      path: "/projects/helsinki/workspaces?lang=en",
      answer: (path, request) =>
        request.method === "GET" && path === "/api/v1/projects/helsinki/workspaces"
          ? jsonResponse(
              list([
                {
                  name: "ws-air",
                  title: "Air quality rework",
                  owner: "viewer@hel.fi",
                  scope: { kind: "project" },
                  expiresAt: "2026-12-01T00:00:00Z",
                  previewState: "running",
                },
              ]),
            )
          : undefined,
    });
    const link = await screen.findByRole("link", { name: "Air quality rework" });
    expect(link).toHaveAttribute("data-row-link");
    const row = link.closest("tr") as HTMLTableRowElement;
    const discard = within(row).queryByRole("button", { name: /Discard/ });
    if (discard) {
      await userEvent.click(discard);
      await userEvent.keyboard("{Escape}");
      expect(window.location.pathname).toBe("/projects/helsinki/workspaces");
    }
    await userEvent.click(within(row).getByText("The whole project"));
    await waitFor(() => {
      expect(window.location.pathname).toBe("/projects/helsinki/spaces");
    });
    expect(new URLSearchParams(window.location.search).get("workspace")).toBe("ws-air");
  });
});

const READER = { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["*"], verbs: ["read"] } }] };

describe("a person who may not change a record still opens it, read only", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  const subscription = manifest("Subscription", "alerts", {
    contextSpaceRef: "air",
    notification: { endpoint: { uri: "https://alerts.example.org/hook" } },
  });
  const answer = (path: string, request: Request) => {
    if (request.method !== "GET") return undefined;
    if (path === "/api/v1/projects/helsinki/subscriptions") return jsonResponse(list([subscription]));
    if (path === "/api/v1/projects/helsinki/subscriptions/alerts") return jsonResponse(subscription);
    if (path === "/api/v1/projects/helsinki/layers") return jsonResponse(list([manifest("Layer", "alerting")]));
    if (path === "/api/v1/projects/helsinki/layers/alerting") return jsonResponse(manifest("Layer", "alerting"));
    return undefined;
  };

  it("a reader gets the filled form, closed, with the reason, and nothing to propose", async () => {
    await renderRoute({ path: "/projects/helsinki/subscriptions/alerts/edit?lang=en", answer, permissions: READER });
    expect(await screen.findByText(/Your role does not permit changing Subscription here/)).toBeInTheDocument();
    expect(screen.getAllByText("alerts").length).toBeGreaterThan(0);
    expect(screen.getByText("Read only")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Propose change" })).toBeNull();
    const fields = await screen.findAllByRole("textbox");
    expect(fields.length).toBeGreaterThan(0);
    for (const field of fields) {
      expect(field).toBeDisabled();
    }
    expect(screen.queryByText(/This form cannot be opened/)).toBeNull();
  });

  it("a reader reads a kind without a form as its manifest, not an editor", async () => {
    await renderRoute({ path: "/projects/helsinki/layers/alerting/edit?lang=en", answer, permissions: READER });
    const text = await screen.findByRole("textbox", { name: "The manifest of alerting" });
    expect(text).toHaveAttribute("readonly");
    expect((text as HTMLTextAreaElement).value).toContain("kind: Layer");
    expect(screen.queryByRole("button", { name: "Propose change" })).toBeNull();
  });

  // Pipelines, data sources and endpoints open their own editors, not the generic dialog; a reader
  // gets the record there too, never the editor whose proposal the server would refuse.
  const OWN_EDITORS = [
    { plural: "pipelines", record: manifest("Pipeline", "ingest", { class: "resident" }), kind: "kind: Pipeline" },
    {
      plural: "datasources",
      record: manifest("DataSource", "feed", { type: "http", http: { url: "https://example.org/feed.json" } }),
      kind: "kind: DataSource",
    },
    {
      plural: "endpoints",
      record: manifest("Endpoint", "public-air", { contextSpaceRef: "air", audience: "public" }),
      kind: "kind: Endpoint",
    },
  ];
  for (const one of OWN_EDITORS) {
    it(`a reader opens a record of ${one.plural} as its manifest, not its editor`, async () => {
      const name = one.record.metadata.name;
      await renderRoute({
        path: `/projects/helsinki/${one.plural}/${name}/edit?lang=en`,
        answer: (path, request) => {
          if (request.method !== "GET") return undefined;
          if (path === `/api/v1/projects/helsinki/${one.plural}`) return jsonResponse(list([one.record]));
          if (path === `/api/v1/projects/helsinki/${one.plural}/${name}`) return jsonResponse(one.record);
          return undefined;
        },
        permissions: READER,
      });
      const text = await screen.findByRole("textbox", { name: `The manifest of ${name}` });
      expect(text).toHaveAttribute("readonly");
      expect((text as HTMLTextAreaElement).value).toContain(one.kind);
      expect(screen.getAllByText(name).length).toBeGreaterThan(0);
      expect(screen.getByText("Read only")).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: /^Propose/ })).toBeNull();
    });
  }

  it("a steward still gets the form to change", async () => {
    await renderRoute({ path: "/projects/helsinki/subscriptions/alerts/edit?lang=en", answer });
    expect((await screen.findAllByText("Edit alerts")).length).toBeGreaterThan(0);
    expect(await screen.findByRole("button", { name: "Propose change" })).toBeInTheDocument();
    expect(screen.queryByText(/Your role does not permit changing/)).toBeNull();
  });
});

describe("the walkers' row-link check", () => {
  const findings = () => pageFindings({ namespaces: [], layout: false }).filter((finding) => finding.startsWith("row-link"));
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("names a row of a table of records that holds no record link", () => {
    document.body.innerHTML = `<main><table data-records=""><caption>Pipelines</caption><tbody>
      <tr><td><a data-row-link="" href="/projects/helsinki/pipelines/ingest/edit">ingest</a></td><td>ready</td></tr>
      <tr><td>orphan</td><td>ready</td></tr>
    </tbody></table></main>`;
    expect(findings()).toEqual(["row-link: Pipelines: orphanready"]);
  });

  it("passes the empty row, a loading table and a table that lists no records", () => {
    document.body.innerHTML = `<main>
      <table data-records=""><caption>Pipelines</caption><tbody><tr><td colspan="3">No pipelines yet</td></tr></tbody></table>
      <table data-records="" aria-busy="true"><caption>Spaces</caption><tbody><tr><td>…</td><td>…</td></tr></tbody></table>
      <table><caption>Fields</caption><tbody><tr><td>name</td><td>Text</td></tr></tbody></table>
    </main>`;
    expect(findings()).toEqual([]);
  });
});
