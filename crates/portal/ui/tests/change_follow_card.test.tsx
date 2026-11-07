/**
 * T-2774 (AG-87, AG-11): a Change the assistant proposed is followed in the conversation to a
 * result that works. Who approves is said; the person approves here when the rules let them and
 * never when they do not; once live, an endpoint is tested by reading a few rows with the
 * person's own session, and a refusal or an empty endpoint says so.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ChangeFollowCard, followedChangeOf } from "../src/pages/apps/ChangeFollowCard";
import { expectNoAxeViolations } from "./page_contract";

const PROJECT = "helsinki";
const ID = "chg-0000003a";

function change(phase: string, { lane = "yellow", author = "someone@hel.fi" } = {}) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ChangeProposal",
    metadata: { name: ID, namespace: PROJECT },
    author: { name: "Someone", email: author },
    createdAt: "2026-09-25T08:00:00Z",
    summary: { key: "change.summary.create", params: { kind: "Endpoint", name: "sample-endpoint" } },
    status: { lane, phase, plan: { create: 1 } },
    planFields: [{ path: "spec.audience", to: "public" }],
  };
}

interface Setup {
  phase?: string;
  lane?: string;
  author?: string;
  verbs?: string[];
  types?: unknown;
  rows?: unknown[];
  tryStatus?: number;
}

function setup({ phase = "PendingApproval", lane, author, verbs = ["read", "propose", "approve"], types, rows, tryStatus = 200 }: Setup = {}) {
  let current = phase;
  const calls: { method: string; path: string; credentials?: string; authorization: string | null }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(request.url, "http://localhost");
      calls.push({
        method: request.method,
        path: `${url.pathname}${url.search}`,
        credentials: init?.credentials,
        authorization: request.headers.get("Authorization"),
      });
      const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json", ...headers } });
      if (url.pathname.endsWith("/auth/me")) return json({ subject: "s", username: "jana", email: "jana@hel.fi", roles: [] });
      if (url.pathname.endsWith("/permissions/me")) {
        return json({
          project: PROJECT,
          bootstrap: false,
          grants: [{ role: "steward", binding: "stewards", scope: "project", rule: { kinds: ["Endpoint"], verbs } }],
        });
      }
      if (url.pathname.endsWith(`/changes/${ID}/approve`) && request.method === "POST") {
        current = "Deploying";
        return json({ changeId: ID, lane: "yellow", status: change(current).status });
      }
      if (url.pathname.endsWith(`/changes/${ID}`)) return json(change(current, { lane, author }));
      if (url.pathname.endsWith("/endpoints/sample-endpoint")) {
        return json({ kind: "Endpoint", metadata: { name: "sample-endpoint" }, spec: { slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa" } });
      }
      if (url.pathname.endsWith("/ngsi-ld/v1/types")) {
        if (tryStatus !== 200) return json({ title: "Forbidden", detail: "Your grants do not reach this endpoint." }, tryStatus);
        return json(types ?? { id: "urn:x", type: "EntityTypeList", typeList: ["BikeHireDockingStation"] });
      }
      if (url.pathname.endsWith("/ngsi-ld/v1/entities")) {
        return json(rows ?? [], 200, { "NGSILD-Results-Count": "7" });
      }
      return json({});
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const opened = vi.fn();
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ChangeFollowCard project={PROJECT} followed={{ changeId: ID, kind: "Endpoint", name: "sample-endpoint" }} onOpenLink={opened} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { calls, opened };
}

const card = () => screen.getByRole("region", { name: "How sample-endpoint goes live" });

describe("a proposed change, followed to a working result", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("follows only a change the step proposed, by an id the Portal mints", () => {
    const ok = { tool: "jc_resource_propose", status: "ok", input: { kind: "Endpoint" }, output: { changeId: ID, lane: "yellow", change: change("PendingApproval") } };
    expect(followedChangeOf(ok)).toEqual({ changeId: ID, kind: "Endpoint", name: "sample-endpoint" });
    expect(followedChangeOf({ ...ok, status: "failed" })).toBeNull();
    expect(followedChangeOf({ ...ok, output: { changeId: "chg-../../admin" } })).toBeNull();
    expect(followedChangeOf({ tool: "search_catalog", status: "ok", output: { items: [] } })).toBeNull();
  });

  it("says who approves and lets a person who may approve do it here", async () => {
    const { calls } = setup();
    expect(await within(card()).findByText(en.agentRun.follow.waiting.replace("{kind}", "Endpoint"))).toBeInTheDocument();
    // F-R2UI2-005: what the agent's Change does is on the card before it can be approved.
    const approveButton = await within(card()).findByRole("button", { name: en.agentRun.follow.approve });
    expect(within(card()).getByRole("table")).toHaveTextContent("public");
    expect(approveButton).toBeInTheDocument();
    await userEvent.click(await within(card()).findByRole("button", { name: en.agentRun.follow.approve }));
    await waitFor(() => {
      expect(calls.some((call) => call.method === "POST" && call.path.endsWith(`/changes/${ID}/approve`))).toBe(true);
    });
    expect(await within(card()).findByText(en.agentRun.follow.approved)).toBeInTheDocument();
    expect(within(card()).getByText(en.agentRun.follow.deploying)).toBeInTheDocument();
  });

  it("offers no approval to a person without the role, and says why", async () => {
    setup({ verbs: ["read", "propose"] });
    expect(await within(card()).findByText(/needs a role binding with `approve`/)).toBeInTheDocument();
    expect(within(card()).queryByRole("button", { name: en.agentRun.follow.approve })).toBeNull();
  });

  it("offers no approval of the person's own change, nor of a red one", async () => {
    setup({ author: "jana@hel.fi" });
    expect(await within(card()).findByText(en.approvals.ownProposal)).toBeInTheDocument();
    expect(within(card()).queryByRole("button", { name: en.agentRun.follow.approve })).toBeNull();
    vi.unstubAllGlobals();
    document.body.innerHTML = "";
    setup({ lane: "red" });
    expect(await within(card()).findByText(en.agentRun.follow.red)).toBeInTheDocument();
    expect(within(card()).queryByRole("button", { name: en.agentRun.follow.approve })).toBeNull();
  });

  it("tests a live endpoint by reading a few rows as the person, and links them to Explore", async () => {
    const { calls, opened } = setup({
      phase: "Applied",
      rows: [
        { id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:toolo", type: "BikeHireDockingStation", freeSlotNumber: { type: "Property", value: 4 } },
        { id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:kamppi", type: "BikeHireDockingStation", freeSlotNumber: { type: "Property", value: 0 } },
      ],
    });
    expect(await within(card()).findByText("Read 2 rows of 7 BikeHireDockingStation from sample-endpoint")).toBeInTheDocument();
    expect(within(card()).getByRole("table")).toBeInTheDocument();
    const read = calls.find((call) => call.path.includes("/ngsi-ld/v1/entities"));
    expect(read?.path).toBe("/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/ngsi-ld/v1/entities?type=BikeHireDockingStation&limit=5&count=true");
    // The person's own session, and no token the page made up.
    expect(read?.credentials).toBe("same-origin");
    expect(read?.authorization).toBeNull();
    await userEvent.click(within(card()).getByRole("link", { name: en.agentRun.follow.test.explore }));
    expect(opened).toHaveBeenCalledWith(`/projects/${PROJECT}/explore?endpoint=sample-endpoint&type=BikeHireDockingStation`);
  });

  it("says a live endpoint holds nothing yet rather than claiming a test passed on rows", async () => {
    setup({ phase: "Applied", types: { typeList: [] } });
    expect(await within(card()).findByText("sample-endpoint is live and holds no entities yet")).toBeInTheDocument();
    expect(within(card()).queryByRole("table")).toBeNull();
  });

  it("shows the endpoint's refusal of the test read, and reads again on request", async () => {
    const { calls } = setup({ phase: "Applied", tryStatus: 403 });
    expect(await within(card()).findByRole("alert")).toHaveTextContent("Your grants do not reach this endpoint.");
    const before = calls.filter((call) => call.path.endsWith("/types")).length;
    await userEvent.click(within(card()).getByRole("button", { name: en.agentRun.follow.test.again }));
    await waitFor(() => {
      expect(calls.filter((call) => call.path.endsWith("/types")).length).toBeGreaterThan(before);
    });
  });

  it("stops at a rejection: nothing to deploy, nothing to test", async () => {
    setup({ phase: "Rejected" });
    expect(await within(card()).findByText(en.agentRun.follow.rejected)).toBeInTheDocument();
    expect(within(card()).queryByText(en.agentRun.follow.test.later)).toBeNull();
  });

  // T-3986: the app builder draws this card only in a model run, which the live walk cannot start (spend guard).
  it("has_no_serious_axe_violation_as_a_model_run_draws_it", async () => {
    setup();
    await within(card()).findByRole("button", { name: en.agentRun.follow.approve });
    await expectNoAxeViolations(document.body);
  });
});
