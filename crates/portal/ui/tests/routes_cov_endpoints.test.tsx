/**
 * EndpointsPage: the one grid of own, shared and gone-reference rows read by every column, the
 * search by name, the row menu's dialogs and how each is dismissed, an endpoint the assistant or
 * the address opens in the editor, and the editor's refusals.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { expectOpen } from "./checks";
import { findFormPage, queryFormPage } from "./formPage";
import { jsonResponse, list, problem, renderRoute } from "./pageHarness";
import { sortEachColumn } from "./routes_cov_grid";
import { greenVerdict, isCheck } from "./verdict";

const API = "joinedcontext.com/v1alpha1";
const PATH = "/projects/helsinki/endpoints";

const endpoint = (project: string, name: string, over: Record<string, unknown> = {}, spec: Record<string, unknown> = {}) => ({
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name, namespace: project, labels: { "joinedcontext.com/space": "air" } },
  spec: {
    contextSpaceRef: "air",
    slug: `slug${name.replaceAll("-", "").padEnd(22, "x")}`.slice(0, 26),
    audience: "organization",
    enabledRepresentations: ["ngsi-ld"],
    ...spec,
  },
  status: { phase: "Live" },
  ...over,
});

const PUBLIC_AIR = endpoint("helsinki", "public-air", {}, { audience: "public" });
const NO_MCP = endpoint("helsinki", "quiet-buses", { status: { phase: "Error" } }, { mcp: false, enabledRepresentations: ["ngsi-ld", "geojson"] });
const SHARED = endpoint("espoo", "espoo-bikes");
const GONE = {
  apiVersion: API,
  kind: "SharedSpaceReference",
  metadata: { name: "vantaa-trams", namespace: "helsinki" },
  spec: { endpointRef: { project: "vantaa", name: "trams" }, alias: "trams" },
  status: { phase: "Live" },
};
const SPACE = {
  apiVersion: API,
  kind: "ContextSpace",
  metadata: { name: "air", namespace: "helsinki" },
  spec: {},
  status: { phase: "Live" },
};

const PROJECTS = () => jsonResponse(list([{ name: "helsinki" }, { name: "espoo" }]));

function answering(rows: { own?: unknown[]; shared?: unknown[]; references?: unknown[] } = {}, extra?: (path: string, request: Request) => Response | Promise<Response> | undefined) {
  return (path: string, request: Request) => {
    const own = extra?.(path, request);
    if (own) return own;
    if (path === "/api/v1/projects/helsinki/endpoints") return jsonResponse(list(rows.own ?? [PUBLIC_AIR, NO_MCP]));
    if (path === "/api/v1/projects/espoo/endpoints") return jsonResponse(list(rows.shared ?? [SHARED]));
    if (path === "/api/v1/projects/helsinki/shared") return jsonResponse(list(rows.references ?? [GONE]));
    if (path === "/api/v1/projects/helsinki/spaces") return jsonResponse(list([SPACE]));
    return undefined;
  };
}

const table = () => screen.findByRole("table", { name: en.endpoints.title });

describe("the endpoints grid", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("sorts by each visible column over own, shared and gone rows", async () => {
    const user = userEvent.setup();
    await renderRoute({ path: PATH, projects: PROJECTS, answer: answering() });
    const grid = await table();
    await within(grid).findByText("espoo-bikes");
    await within(grid).findByText("vantaa-trams");
    const sorted = await sortEachColumn(user, grid);
    expect(Object.keys(sorted)).toEqual(expect.arrayContaining([en.endpoints.source.label, en.endpoints.field.audience, en.endpoints.calls.label, en.endpoints.field.phase]));
    for (const rows of Object.values(sorted)) expect(rows).toHaveLength(4);
    // Phase reads empty for a row that is not this project's, so those sort first.
    expect(sorted[en.endpoints.field.phase].at(-1)).toBe("quiet-buses");
  });

  it("reads the hidden columns too when a search matches no row", async () => {
    const user = userEvent.setup();
    await renderRoute({ path: PATH, projects: PROJECTS, answer: answering() });
    const grid = await table();
    await within(grid).findByText("espoo-bikes");
    await user.type(screen.getAllByRole("searchbox").at(-1) as HTMLElement, "zzz-nothing");
    expect(await screen.findByText(en.grid.noMatch)).toBeInTheDocument();
  });

  it("narrows by the name typed in the header, names what matched nothing and clears from the empty state", async () => {
    const user = userEvent.setup();
    await renderRoute({ path: PATH, projects: PROJECTS, answer: answering() });
    const grid = await table();
    await within(grid).findByText("vantaa-trams");
    const find = screen.getByRole("searchbox", { name: en.endpoints.find });
    await user.type(find, "buses");
    await waitFor(() => expect(within(grid).queryByText("public-air")).toBeNull());
    expect(within(grid).getByText("quiet-buses")).toBeInTheDocument();
    // A gone reference is found by its own name.
    await user.clear(find);
    await user.type(find, "trams");
    await waitFor(() => expect(within(grid).queryByText("quiet-buses")).toBeNull());
    expect(within(grid).getByText("vantaa-trams")).toBeInTheDocument();
    await user.clear(find);
    await user.type(find, "nothing-like-this");
    expect(await screen.findByText(en.endpoints.searchNoMatch.replace("{{text}}", "nothing-like-this"))).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.spaces.inside.clearSearch }));
    expect(await screen.findByText("quiet-buses")).toBeInTheDocument();
    expect(find).toHaveValue("");
  });

  it("opens the connector dialog from the header", async () => {
    const user = userEvent.setup();
    await renderRoute({ path: PATH, projects: PROJECTS, answer: answering() });
    await user.click(await screen.findByRole("button", { name: en.endpoints.hub.button }));
    expect(await screen.findByRole("dialog", { name: en.endpoints.hub.title })).toBeInTheDocument();
  });
});

describe("the row menu of an endpoint", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  async function openMenu(name = "public-air") {
    const user = userEvent.setup();
    const grid = await table();
    const row = (await within(grid).findByText(name)).closest("tr") as HTMLElement;
    await user.click(within(row).getByRole("button", { name: /More actions/ }));
    return user;
  }

  it("opens the endpoint's own page", async () => {
    await renderRoute({ path: PATH, projects: PROJECTS, answer: answering() });
    const user = await openMenu();
    await user.click(await screen.findByRole("menuitem", { name: en.resourceList.open }));
    await waitFor(() => expect(window.location.pathname).toBe("/projects/helsinki/endpoints/public-air"));
  });

  it("copies the public address to the clipboard", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await renderRoute({ path: PATH, projects: PROJECTS, answer: answering() });
    const user = await openMenu();
    await user.click(await screen.findByRole("menuitem", { name: en.endpoints.copyUrl }));
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(String(writeText.mock.calls[0]).includes(PUBLIC_AIR.spec.slug)).toBe(true);
  });

  it.each([
    [en.grid.yaml.action],
    [en.saveAs.button],
    [en.workspaces.open.action],
    [en.resourceDelete.button],
  ])("opens %s and leaves nothing open and nothing written after Escape", async (action) => {
    const { calls } = await renderRoute({ path: PATH, projects: PROJECTS, answer: answering() });
    const user = await openMenu();
    await user.click(await screen.findByRole("menuitem", { name: action }));
    await screen.findByRole("dialog");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(calls().filter((call) => !call.startsWith("GET"))).toEqual([]);
  });
});

describe("the endpoint editor", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("opens the endpoint the address names, filled from the list, and closes without writing", async () => {
    const { calls } = await renderRoute({ path: `${PATH}?draft=public-air`, projects: PROJECTS, answer: answering() });
    const form = await findFormPage();
    await waitFor(() => expect(within(form).getByDisplayValue("public-air")).toBeInTheDocument());
    expect(within(form).getByText(PUBLIC_AIR.spec.slug)).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(queryFormPage()).toBeNull());
    expect(calls().filter((call) => call.startsWith("POST") || call.startsWith("PUT")).filter((call) => !call.includes("/drafts/"))).toEqual([]);
  });

  it("says why the server refused the proposal and keeps the form", async () => {
    await renderRoute({
      path: `${PATH}?draft=public-air`,
      projects: PROJECTS,
      answer: answering({}, async (path, request) => {
        const url = new URL(request.url);
        if (path === "/api/v1/projects/helsinki/endpoints/public-air" && isCheck(request, url)) {
          return jsonResponse(await greenVerdict(request, { valid: true }));
        }
        if (path === "/api/v1/projects/helsinki/endpoints/public-air" && request.method === "PUT") {
          return problem(403, "Your role may not change the audience of 'public-air'.");
        }
        return undefined;
      }),
    });
    const form = await findFormPage();
    await waitFor(() => expect(within(form).getByDisplayValue("public-air")).toBeInTheDocument());
    await userEvent.click(within(form).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() => expectOpen(within(form).getByRole("button", { name: en.endpoints.propose })));
    await userEvent.click(within(form).getByRole("button", { name: en.endpoints.propose }));
    expect(await within(form).findByText(/Your role may not change the audience/)).toBeInTheDocument();
    expect(queryFormPage()).not.toBeNull();
  });

  it("opens a person who may not propose on the record read only, and Escape closes it", async () => {
    await renderRoute({
      path: `${PATH}?draft=public-air`,
      projects: PROJECTS,
      permissions: { project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["Endpoint"], verbs: ["read"] } }] },
      answer: answering(),
    });
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByRole("button", { name: en.endpoints.propose })).toBeNull();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});
