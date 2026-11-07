/**
 * The indicator pipeline card (AG-74): what the drafted pipeline reads, writes and when, what its
 * test said, and, for an indicator space that is new, its drafts proposed in order with the
 * person's session, stopping at the first refusal.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { RouterProvider, createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { KpiPipelineCard, kpiPipelineOf } from "../src/pages/apps/KpiPipelineCard";
import type { KpiPipeline } from "../src/pages/apps/KpiPipelineCard";
import { answeringChecks, checksSoFar } from "./checks";

const OUTPUT = {
  name: "free-bikes",
  title: "Free bikes <b>now</b>",
  formula: "sum(availableBikeNumber) over BikeHireDockingStation",
  trigger: "on every change of availableBikeNumber",
  sourceEndpoint: "helsinki-all",
  sourceSpace: "helsinki",
  targetSpace: "transportation-kpi",
  targetSlug: "newslugnewslugnewslugnewsl",
  verdict: { ok: false, untested: "no pipeline runner is configured" },
  runnerAudience: "newslugnewslugnewslugnewsl",
  drafts: [
    { kind: "ContextSpace", name: "transportation-kpi", plural: "contextspaces", manifest: { kind: "ContextSpace", metadata: { name: "transportation-kpi" } } },
    { kind: "Endpoint", name: "transportation-kpi", plural: "endpoints", manifest: { kind: "Endpoint", metadata: { name: "transportation-kpi" } } },
    { kind: "Policy", name: "transportation-kpi-read", plural: "policies", manifest: { kind: "Policy", metadata: { name: "transportation-kpi-read" } } },
  ],
};

function renderCard(pipeline: KpiPipeline) {
  const rootRoute = createRootRoute();
  const home = createRoute({ getParentRoute: () => rootRoute, path: "/", component: () => <KpiPipelineCard project="helsinki" pipeline={pipeline} /> });
  const plural = createRoute({ getParentRoute: () => rootRoute, path: "/projects/$project/$plural", component: () => <p>list</p> });
  const router = createRouter({ routeTree: rootRoute.addChildren([home, plural]) });
  render(
    <I18nextProvider i18n={i18n}>
      <RouterProvider router={router} />
    </I18nextProvider>,
  );
  return router;
}

describe("the indicator pipeline card", () => {
  beforeEach(async () => {
    window.history.pushState({}, "", "/");
    document.cookie = "__Host-jc_csrf=csrf-xyz; Secure; path=/";
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reads only what the Portal wrote", () => {
    const pipeline = kpiPipelineOf(OUTPUT);
    expect(pipeline?.drafts).toHaveLength(3);
    expect(pipeline?.verdict).toEqual({ ok: false, untested: "no pipeline runner is configured", findings: [] });
    expect(kpiPipelineOf({ name: "x" })).toBeNull();
    expect(kpiPipelineOf(null)).toBeNull();
  });

  it("shows the route, the trigger, an untested verdict as untested, and the title as text", async () => {
    renderCard(kpiPipelineOf(OUTPUT) as KpiPipeline);
    const card = await screen.findByRole("region", { name: en.agentRun.kpiPipeline.title });
    expect(within(card).getByText("helsinki-all → transportation-kpi")).toBeInTheDocument();
    expect(within(card).getByText("on every change of availableBikeNumber")).toBeInTheDocument();
    expect(within(card).getByTestId("kpi-pipeline-verdict")).toHaveTextContent("Not tested yet: no pipeline runner is configured");
    expect(within(card).getByText("Free bikes <b>now</b>")).toBeInTheDocument();
    expect(document.querySelector("b")).toBeNull();
    expect(within(card).getByText(/newslugnewslugnewslugnewsl/)).toBeInTheDocument();
  });

  it("proposes the new space's drafts in order and stops at the first refusal", async () => {
    const calls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string) => {
        const url = typeof input === "string" ? input : input.url;
        calls.push(new URL(url, "http://portal.test").pathname);
        if (url.includes("/policies")) {
          return new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "may not propose a Policy" }), {
            status: 403,
            headers: { "content-type": "application/problem+json" },
          });
        }
        return new Response(JSON.stringify({ id: "c1", branch: "b", lane: "yellow" }), { status: 201, headers: { "content-type": "application/json" } });
      }),
    );
    vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
    renderCard(kpiPipelineOf(OUTPUT) as KpiPipeline);
    await userEvent.click(await screen.findByRole("button", { name: en.agentRun.kpiPipeline.propose }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Policy transportation-kpi-read was not proposed: may not propose a Policy");
    expect(calls).toEqual([
      "/api/v1/projects/helsinki/contextspaces",
      "/api/v1/projects/helsinki/endpoints",
      "/api/v1/projects/helsinki/policies",
    ]);
    // Checked before it was proposed (PF-57, T-0956).
    expect(checksSoFar().some((check) => check.includes("POST /api/v1/projects/"))).toBe(true);
  });

  it("with an existing space offers only the pipelines page and says when the test passed", async () => {
    renderCard(kpiPipelineOf({ ...OUTPUT, drafts: [], runnerAudience: null, verdict: { ok: true } }) as KpiPipeline);
    expect(await screen.findByText(en.agentRun.kpiPipeline.tested)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.agentRun.kpiPipeline.propose })).toBeNull();
    expect(screen.getByRole("link", { name: en.agentRun.kpiPipeline.open })).toBeInTheDocument();
  });
});
