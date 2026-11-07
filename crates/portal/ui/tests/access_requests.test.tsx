// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/pipelines/AccessRequests.tsx.
/**
 * T-4863, PL-90…PL-93: a pipeline asks another space's owner for access, naming every type and
 * attribute; the owner approves, declines with a reason, or revokes, each a call of the owning
 * project, and approving shows the Change it proposed.
 */
import type { JSX } from "react";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AccessRequestInbox, PipelineAccessRequests, requestBody, useAccessAsk } from "../src/pages/pipelines/AccessRequests";
import { RecordTable } from "../src/pages/pipelines/WorkbenchTarget";
import type { CheckFinding } from "../src/pages/pipelines/WorkbenchTarget";
import type { AccessRequestView } from "../src/pages/pipelines/AccessRequests";
import { InRouter } from "./pageHarness";

const L = en.pipelines.access;

const request = (state: AccessRequestView["state"]): AccessRequestView => ({
  id: "ar-0a1b2c3d4e5f",
  pipeline: { project: "city-kpis", name: "air-index" },
  account: "city-kpis/pl-air-index",
  endpoint: { project: "helsinki", name: "air-quality" },
  space: "air",
  operations: "read",
  types: [{ type: "AirQualityObserved", attrs: ["pm10", "pm25"] }],
  until: "2027-01-31T23:59:59Z",
  reason: "air index for the city dashboard",
  state,
  requestedBy: "jana",
  requestedAt: "2026-10-20T19:40:00Z",
  decidedBy: state === "pending" ? null : "olli",
  decidedAt: null,
  decisionReason: state === "declined" ? "use the open endpoint" : null,
  change: null,
});

describe("the request a form builds (PL-90)", () => {
  it("asks for what is missing first", () => {
    const form = { project: "helsinki", endpoint: "air-quality", operations: "read" as const, rows: [{ type: "AirQualityObserved", attrs: ["pm10"] }], until: "", reason: "why" };
    expect(requestBody(form)).toEqual({
      body: { endpoint: { project: "helsinki", name: "air-quality" }, operations: "read", types: [{ type: "AirQualityObserved", attrs: ["pm10"] }], reason: "why" },
    });
    expect(requestBody({ ...form, endpoint: " " })).toEqual({ missing: "endpoint" });
    expect(requestBody({ ...form, rows: [{ type: "AirQualityObserved", attrs: [] }] })).toEqual({ missing: "types" });
    expect(requestBody({ ...form, rows: [{ type: "", attrs: [] }] })).toEqual({ missing: "types" });
    expect(requestBody({ ...form, reason: "" })).toEqual({ missing: "reason" });
    expect((requestBody({ ...form, until: "2027-01-31" }) as { body: { until: string } }).body.until).toMatch(/^2027-0[12]-/);
  });
});

describe("access requests on the pages (PL-90, PL-91, PL-93)", () => {
  let sent: { path: string; body: unknown }[];
  const offered = [
    { project: "helsinki", name: "air-quality", space: "air", types: [{ type: "AirQualityObserved", attrs: ["pm10", "pm25", "no2"] }] },
    { project: "traffic", name: "counts", space: "roads", types: [] },
  ];
  function serve(items: AccessRequestView[], endpoints: unknown[] = offered, refusal?: { status: number; detail: string }) {
    sent = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const req = input as Request;
        const url = new URL(req.url);
        const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
        if (req.method === "POST") {
          const text = await req.text();
          sent.push({ path: url.pathname, body: text ? JSON.parse(text) : null });
          if (refusal) {
            return new Response(JSON.stringify({ title: "Invalid Request", status: refusal.status, detail: refusal.detail }), {
              status: refusal.status,
              headers: { "Content-Type": "application/problem+json" },
            });
          }
          if (url.pathname.endsWith("/approve") || url.pathname.endsWith("/revoke")) {
            return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-00000042", namespace: "helsinki" }, status: { lane: "red", phase: "PendingApproval", plan: { create: 1, update: 1, delete: 0 } } }, 202);
          }
          return json(request("pending"), 201);
        }
        if (url.pathname.endsWith("/access-requests/endpoints")) return json({ items: endpoints });
        if (url.pathname.endsWith("/access-requests")) return json({ items });
        return json({ items: [] });
      }),
    );
  }
  const wrap = (node: JSX.Element) =>
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <InRouter>{node}</InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );

  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("files a request naming every type and attribute, and refuses an empty one in place", async () => {
    serve([]);
    wrap(<PipelineAccessRequests project="city-kpis" name="air-index" />);
    expect(await screen.findByText(L.none)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: L.request }));
    await userEvent.click(screen.getByRole("button", { name: L.send }));
    expect(await screen.findByRole("alert")).toHaveTextContent(L.missing.endpoint);
    expect(sent).toHaveLength(0);
    // The Endpoint, its type and its attributes are picked from what the server offers, never typed.
    await userEvent.click(screen.getByRole("combobox", { name: L.endpoint }));
    expect(await screen.findByRole("option", { name: /counts/ })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("option", { name: /air-quality/ }));
    await userEvent.click(screen.getByRole("combobox", { name: L.type }));
    await userEvent.click(await screen.findByRole("option", { name: /AirQualityObserved/ }));
    await userEvent.click(screen.getByRole("combobox", { name: L.attrs }));
    await userEvent.click(await screen.findByRole("option", { name: /pm10/ }));
    await userEvent.click(await screen.findByRole("option", { name: /pm25/ }));
    await userEvent.type(screen.getByLabelText(new RegExp(L.reason)), "air index");
    await userEvent.click(screen.getByRole("button", { name: L.send }));
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0].path).toBe("/api/v1/projects/city-kpis/pipelines/air-index/access-requests");
    expect(sent[0].body).toEqual({
      endpoint: { project: "helsinki", name: "air-quality" },
      operations: "read",
      types: [{ type: "AirQualityObserved", attrs: ["pm10", "pm25"] }],
      reason: "air index",
    });
    expect(await screen.findByText(L.sent)).toBeInTheDocument();
  });

  it("opens prefilled from a missing grant and sends what the person completes (T-5369)", async () => {
    serve([]);
    wrap(
      <PipelineAccessRequests
        project="city-kpis"
        name="air-index"
        prefill={{ endpoint: "helsinki/air-quality", operations: "write", type: "AirQualityObserved" }}
      />,
    );
    expect(await screen.findByText(L.prefilled)).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: L.operations.write })).toBeChecked();
    // The offered Endpoint's model gives the attributes; the person picks them and says why.
    await userEvent.click(screen.getByRole("combobox", { name: L.attrs }));
    await userEvent.click(await screen.findByRole("option", { name: /no2/ }));
    await userEvent.type(screen.getByLabelText(new RegExp(L.reason)), "write the air index");
    await userEvent.click(screen.getByRole("button", { name: L.send }));
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0].body).toEqual({
      endpoint: { project: "helsinki", name: "air-quality" },
      operations: "write",
      types: [{ type: "AirQualityObserved", attrs: ["no2"] }],
      reason: "write the air index",
    });
  });

  function Checked({ pipeline, findings }: { pipeline?: string; findings: CheckFinding[] }): JSX.Element {
    const access = useAccessAsk("city-kpis", pipeline, true);
    return (
      <>
        <RecordTable index={0} record={{ id: "urn:ngsi-ld:AirQualityObserved:1", type: "AirQualityObserved" }} verdict={{ ok: false, findings }} ask={access.ask} />
        {access.form}
      </>
    );
  }
  const missing = (project: string, name: string, operations: "read" | "write"): CheckFinding => ({
    source: "endpoint",
    rule: operations === "write" ? "policy" : "sourceEndpoint",
    field: "",
    reason: `no grant on ${name}`,
    access: { project, name, operations, type: "AirQualityObserved" },
  });

  it("names a missing grant by the space the server offers and opens the form with it (T-5369)", async () => {
    serve([]);
    wrap(<Checked pipeline="air-index" findings={[missing("helsinki", "air-quality", "write"), missing("parks", "trees", "read")]} />);
    // The space only from the server's list for this person; an Endpoint it does not offer by name.
    expect(await screen.findByText(L.noAccess.replace("{where}", "air"))).toBeInTheDocument();
    expect(screen.getByText(L.noAccess.replace("{where}", "parks/trees"))).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: L.requestFor.replace("{endpoint}", "parks/trees") }));
    const form = await screen.findByTestId("workbench-access");
    expect(within(form).getByRole("radio", { name: L.operations.read })).toBeChecked();
    expect(within(form).getByText(L.typedTypeHelp)).toBeInTheDocument();
    await userEvent.type(within(form).getByRole("combobox", { name: L.attrs }), "height{Enter}");
    await userEvent.type(within(form).getByLabelText(new RegExp(L.reason)), "tree index");
    await userEvent.click(within(form).getByRole("button", { name: L.send }));
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0].body).toEqual({
      endpoint: { project: "parks", name: "trees" },
      operations: "read",
      types: [{ type: "AirQualityObserved", attrs: ["height"] }],
      reason: "tree index",
    });
  });

  it("asks to propose a pipeline first when it has no account yet (T-5369)", async () => {
    serve([]);
    wrap(<Checked findings={[missing("helsinki", "air-quality", "write")]} />);
    const button = await screen.findByRole("button", { name: L.requestFor.replace("{endpoint}", "helsinki/air-quality") });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText(L.noAccess.replace("{where}", "helsinki/air-quality"))).toBeInTheDocument();
    expect(screen.queryByTestId("workbench-access")).toBeNull();
  });

  it("asks for an Endpoint the list does not offer by its typed name, with typed types the owners check", async () => {
    serve([]);
    wrap(<PipelineAccessRequests project="city-kpis" name="air-index" />);
    await userEvent.click(await screen.findByRole("button", { name: L.request }));
    expect(screen.getByText(L.endpointHelp)).toBeInTheDocument();
    await userEvent.type(screen.getByRole("combobox", { name: L.endpoint }), "Not a name{Enter}");
    expect(await screen.findByText(L.endpointTypedBad.replace("{name}", "not a name"))).toBeInTheDocument();
    await userEvent.clear(screen.getByRole("combobox", { name: L.endpoint }));
    await userEvent.type(screen.getByRole("combobox", { name: L.endpoint }), "parks/trees{Enter}");
    expect(await screen.findByText(L.typedTypeHelp)).toBeInTheDocument();
    await userEvent.type(screen.getByRole("combobox", { name: L.type }), "TreeObserved{Enter}");
    await userEvent.type(screen.getByRole("combobox", { name: L.attrs }), "height{Enter}");
    await userEvent.type(screen.getByLabelText(new RegExp(L.reason)), "tree index");
    await userEvent.click(screen.getByRole("button", { name: L.send }));
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0].body).toEqual({
      endpoint: { project: "parks", name: "trees" },
      operations: "read",
      types: [{ type: "TreeObserved", attrs: ["height"] }],
      reason: "tree index",
    });
  });

  it("says why an Endpoint offers no type, and how to go on when none is offered at all", async () => {
    serve([]);
    wrap(<PipelineAccessRequests project="city-kpis" name="air-index" />);
    await userEvent.click(await screen.findByRole("button", { name: L.request }));
    await userEvent.click(screen.getByRole("combobox", { name: L.endpoint }));
    await userEvent.click(await screen.findByRole("option", { name: /counts/ }));
    expect(screen.getByText(L.noTypes)).toBeInTheDocument();
  });

  it("lists no Endpoint when the server offers none", async () => {
    serve([], []);
    wrap(<PipelineAccessRequests project="city-kpis" name="air-index" />);
    await userEvent.click(await screen.findByRole("button", { name: L.request }));
    await userEvent.click(screen.getByRole("combobox", { name: L.endpoint }));
    expect(await screen.findByText(L.noEndpoints)).toBeInTheDocument();
  });

  it("shows the server's refusal of a value beside the form", async () => {
    serve([], offered, { status: 400, detail: "types[0].attrs: 'pm25' is no attribute of AirQualityObserved at endpoint 'helsinki/air-quality'" });
    wrap(<PipelineAccessRequests project="city-kpis" name="air-index" />);
    await userEvent.click(await screen.findByRole("button", { name: L.request }));
    await userEvent.click(screen.getByRole("combobox", { name: L.endpoint }));
    await userEvent.click(await screen.findByRole("option", { name: /air-quality/ }));
    await userEvent.click(screen.getByRole("combobox", { name: L.type }));
    await userEvent.click(await screen.findByRole("option", { name: /AirQualityObserved/ }));
    await userEvent.click(screen.getByRole("combobox", { name: L.attrs }));
    await userEvent.click(await screen.findByRole("option", { name: /pm25/ }));
    await userEvent.type(screen.getByLabelText(new RegExp(L.reason)), "air index");
    await userEvent.click(screen.getByRole("button", { name: L.send }));
    expect(await screen.findByRole("alert")).toHaveTextContent("types[0].attrs: 'pm25'");
  });

  it("shows the requester where each request stands and the owner's reason", async () => {
    serve([request("declined")]);
    wrap(<PipelineAccessRequests project="city-kpis" name="air-index" />);
    const row = (await screen.findByText("helsinki/air-quality")).closest("li") as HTMLElement;
    expect(row).toHaveTextContent(L.state.declined);
    expect(row).toHaveTextContent("olli: use the open endpoint");
    expect(row).toHaveTextContent("AirQualityObserved (pm10, pm25)");
  });

  it("lets the owner approve a pending request and shows the Change, and declines only with a reason", async () => {
    serve([request("pending")]);
    wrap(<AccessRequestInbox project="helsinki" space="air" />);
    const row = (await screen.findByText("city-kpis/pl-air-index")).closest("li") as HTMLElement;
    expect(within(row).getByRole("button", { name: "Decline access for city-kpis/pl-air-index" })).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(within(row).getByRole("button", { name: "Approve access for city-kpis/pl-air-index" }));
    await waitFor(() => expect(sent.map((s) => s.path)).toEqual(["/api/v1/projects/helsinki/access-requests/ar-0a1b2c3d4e5f/approve"]));
    expect(await screen.findByText("chg-00000042")).toBeInTheDocument();
  });

  it("revokes an approved request with the owner's reason and lists nothing of another space", async () => {
    serve([request("approved"), { ...request("pending"), id: "ar-111111111111", space: "traffic", account: "x/pl-y" }]);
    wrap(<AccessRequestInbox project="helsinki" space="air" />);
    const row = (await screen.findByText("city-kpis/pl-air-index")).closest("li") as HTMLElement;
    expect(screen.queryByText("x/pl-y")).not.toBeInTheDocument();
    await userEvent.type(within(row).getByLabelText(L.decisionReason), "the index moved");
    await userEvent.click(within(row).getByRole("button", { name: "Revoke access of city-kpis/pl-air-index" }));
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]).toEqual({ path: "/api/v1/projects/helsinki/access-requests/ar-0a1b2c3d4e5f/revoke", body: { reason: "the index moved" } });
  });
});
