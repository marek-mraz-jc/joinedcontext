/**
 * T-5531, API/01 §57: the Endpoint page's "Who may do what". Every principal against each type
 * and read, write, delete and history, a cell's attributes, condition and Policy one click away,
 * the graph principal → Policy → Endpoint → type, and the refusal and empty states.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { App } from "../src/App";
import { AccessMatrix, accessMatrixQuery, cellState, graphNodes } from "../src/components/endpoints/AccessMatrix";
import type { Matrix, MatrixCell } from "../src/components/endpoints/AccessMatrix";

const ENDPOINT = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Endpoint",
  metadata: { name: "bikes-ops", namespace: "helsinki" },
  spec: { slug: "helsinki-bikes-ops", contextSpaceRef: "bikes" },
};

export const MATRIX: Matrix = {
  endpoint: "bikes-ops",
  space: "bikes",
  principals: [
    { kind: "group", name: "bike-stewards", via: [{ kind: "policy", name: "stewards-write" }] },
    { kind: "public", name: "public", via: [{ kind: "policy", name: "public-read" }] },
  ],
  types: ["BikeHireDockingStation"],
  cells: [
    {
      principal: "group:bike-stewards",
      type: "BikeHireDockingStation",
      action: "write",
      effect: "permit",
      attributes: ["status"],
      constraint: 'q: operator=="HSL"',
      decidedBy: "stewards-write",
    },
    {
      principal: "public:public",
      type: "BikeHireDockingStation",
      action: "read",
      effect: "permit",
      attributes: "*",
      constraint: null,
      decidedBy: "public-read",
    },
    {
      principal: "public:public",
      type: "BikeHireDockingStation",
      action: "history",
      effect: "permit",
      attributes: "*",
      constraint: null,
      decidedBy: "public-read",
    },
  ],
  edges: [
    { from: "principal:group:bike-stewards", to: "policy:stewards-write" },
    { from: "policy:stewards-write", to: "endpoint:bikes-ops" },
    { from: "principal:public:public", to: "policy:public-read" },
    { from: "policy:public-read", to: "endpoint:bikes-ops" },
    { from: "endpoint:bikes-ops", to: "type:BikeHireDockingStation" },
  ],
};

const LIST = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });

function stub(answer: { status: number; body: unknown }): string[] {
  const asked: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      if (url.pathname.endsWith("/auth/me")) return json({ subject: "s", username: "anna", roles: [] });
      if (url.pathname === "/api/v1/projects") return json({ items: [{ name: "helsinki" }] });
      if (url.pathname.endsWith("/permissions/me")) return json({ project: "helsinki", bootstrap: true, grants: [] });
      if (url.pathname === "/api/v1/projects/helsinki/endpoints/bikes-ops") return json(ENDPOINT);
      if (url.pathname.endsWith("/access/matrix")) {
        asked.push(url.search);
        return json(answer.body, answer.status);
      }
      return json(LIST([]));
    }),
  );
  return asked;
}

function renderPage() {
  window.history.pushState({}, "", "/projects/helsinki/endpoints/bikes-ops");
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

async function section() {
  const heading = await screen.findByRole("heading", { name: "Who may do what" }, { timeout: 5000 });
  return heading.closest("section") as HTMLElement;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("cellState", () => {
  const permit = (over: Partial<MatrixCell> = {}): MatrixCell => ({
    principal: "public:public",
    type: "T",
    action: "read",
    effect: "permit",
    attributes: "*",
    constraint: null,
    decidedBy: null,
    ...over,
  });
  it("reads allow, partial and deny from the cells", () => {
    expect(cellState([])).toBe("deny");
    expect(cellState([permit()])).toBe("allow");
    expect(cellState([permit({ attributes: ["status"] })])).toBe("partial");
    expect(cellState([permit({ constraint: "q: a==1" })])).toBe("partial");
    expect(cellState([permit(), permit({ effect: "prohibit", attributes: ["owner"] })])).toBe("partial");
    expect(cellState([permit(), permit({ effect: "prohibit" })])).toBe("deny");
  });
});

describe("graphNodes", () => {
  it("puts each step of the path in its own column", () => {
    const nodes = graphNodes(MATRIX);
    const column = (id: string) => nodes.find((node) => node.id === id)?.column;
    expect(column("principal:group:bike-stewards")).toBe(0);
    expect(column("policy:public-read")).toBe(1);
    expect(column("endpoint:bikes-ops")).toBe(2);
    expect(column("type:BikeHireDockingStation")).toBe(3);
    expect(nodes.filter((node) => node.column === 0).map((node) => node.y)).toEqual([0, 88]);
  });
});

describe("accessMatrixQuery", () => {
  it("refuses an answer that is no matrix instead of drawing it", async () => {
    stub({ status: 200, body: { items: [] } });
    const query = accessMatrixQuery("helsinki", "bikes-ops", "group");
    await expect(query.queryFn({ signal: new AbortController().signal })).rejects.toThrow("no access matrix");
  });
});

describe("AccessMatrix alone", () => {
  it("asks once per kind picked and says when nobody is named", async () => {
    const asked = stub({ status: 200, body: { ...MATRIX, principals: [], cells: [], edges: [], types: [] } });
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <AccessMatrix project="helsinki" name="bikes-ops" />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(await screen.findByText("No Policy names anyone on this Endpoint yet")).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByLabelText("Show"), "serviceAccount");
    await waitFor(() => expect(asked).toEqual(["", "?kind=serviceAccount"]));
  });
});

describe("Who may do what", () => {
  it("shows every principal against each action, a partial cell's detail and the graph", async () => {
    const asked = stub({ status: 200, body: MATRIX });
    renderPage();
    const box = await section();
    const table = await within(box).findByRole("group", { name: "Who may do what on bikes-ops" });
    expect(within(table).getByText("bike-stewards")).toBeInTheDocument();
    expect(within(table).getByRole("button", { name: "The public, Read BikeHireDockingStation: Allowed" })).toBeInTheDocument();
    expect(within(table).getByRole("button", { name: "The public, Delete BikeHireDockingStation: Refused" })).toBeInTheDocument();

    await userEvent.click(within(table).getByRole("button", { name: "bike-stewards, Write BikeHireDockingStation: In part" }));
    expect(within(box).getByText("Only these attributes: status")).toBeInTheDocument();
    expect(within(box).getByText('q: operator=="HSL"')).toBeInTheDocument();
    expect(within(box).getByRole("link", { name: "stewards-write" })).toHaveAttribute(
      "href",
      "/projects/helsinki/policies/stewards-write/edit",
    );

    await userEvent.selectOptions(within(box).getByLabelText("Show"), "group");
    await waitFor(() => expect(asked).toContain("?kind=group"));

    await userEvent.click(within(box).getByRole("tab", { name: "Graph" }));
    const graph = within(box).getByRole("group", { name: "How each principal reaches bikes-ops" });
    expect(within(graph).getByText("stewards-write")).toBeInTheDocument();
    expect(within(graph).getByText("BikeHireDockingStation")).toBeInTheDocument();
  });

  it("says who may read it when the server refuses", async () => {
    stub({
      status: 403,
      body: {
        type: "https://joinedcontext.com/errors/forbidden",
        title: "Forbidden",
        status: 403,
        detail: "the access matrix is for organization administrators and for whoever reads Policies in the Endpoint's space",
      },
    });
    renderPage();
    const box = await section();
    expect(await within(box).findByText(/whoever reads Policies/)).toBeInTheDocument();
  });

  it("says what to do next when no Policy names anyone", async () => {
    stub({ status: 200, body: { ...MATRIX, principals: [], cells: [], edges: [], types: [] } });
    renderPage();
    const box = await section();
    expect(await within(box).findByText("No Policy names anyone on this Endpoint yet")).toBeInTheDocument();
  });
});
