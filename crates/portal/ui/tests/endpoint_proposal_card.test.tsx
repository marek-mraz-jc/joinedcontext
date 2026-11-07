/**
 * The endpoint proposal card (T-0587, EP-72, UI-45): what the assistant drafted, the slug as
 * read-only text, hidden attributes struck through, the lane, and one button that leaves the
 * form's prefill behind and opens the endpoints page.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { takePrefill } from "../src/assistant/state";
import { EndpointProposalCard, proposalOf } from "../src/pages/apps/EndpointProposalCard";
import type { Proposal } from "../src/pages/apps/EndpointProposalCard";
import { expectNoAxeViolations } from "./page_contract";

const SLUG = "k7m2p9q4r6s8t3v5w7x2y4z6a8";

const PROPOSAL: Proposal = {
  lane: "yellow",
  slug: SLUG,
  endpoint: {
    metadata: { name: "bikes-regional-transport", title: { en: "City bikes for the regional transport team" } },
    spec: {
      contextSpaceRef: "helsinki",
      slug: SLUG,
      audience: "project-list",
      allowedProjects: ["regional-transport"],
      enabledRepresentations: ["ngsi-ld", "csv"],
      projection: { hiddenAttributes: ["maintenanceNote", "<b>internalCode</b>"] },
    },
  },
  policies: [
    {
      kind: "Policy",
      spec: {
        assignee: { kind: "group", id: "regional-transport" },
        operations: ["retrieveOps"],
        information: [{ entities: [{ type: "BikeHireDockingStation" }] }],
      },
    },
  ],
  prefill: {
    name: "bikes-regional-transport",
    contextSpaceRef: "helsinki",
    slug: SLUG,
    audience: "project-list",
    allowedProjects: ["regional-transport"],
    enabledRepresentations: ["ngsi-ld", "csv"],
    hiddenAttributes: ["maintenanceNote", "<b>internalCode</b>"],
  },
};

function renderCard(proposal: Proposal = PROPOSAL) {
  const rootRoute = createRootRoute();
  const home = createRoute({
    getParentRoute: () => rootRoute,
    path: "/",
    component: () => <EndpointProposalCard project="helsinki" proposal={proposal} />,
  });
  const endpoints = createRoute({
    getParentRoute: () => rootRoute,
    path: "/projects/$project/$plural",
    component: () => <p>endpoints page</p>,
  });
  const router = createRouter({ routeTree: rootRoute.addChildren([home, endpoints]) });
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <RouterProvider router={router} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return router;
}

describe("the endpoint proposal card", () => {
  beforeEach(async () => {
    window.history.pushState({}, "", "/");
    sessionStorage.clear();
    await i18n.changeLanguage("en");
  });

  it("shows the draft: title, read-only slug, audience, representations, struck hidden attributes and the lane", async () => {
    renderCard();
    const card = await screen.findByRole("region", { name: en.agentRun.proposal.title });
    expect(within(card).getByText("City bikes for the regional transport team")).toBeInTheDocument();
    expect(within(card).getByText(SLUG)).toBeInTheDocument();
    expect(within(card).queryByRole("textbox")).toBeNull();
    expect(within(card).getByText("project-list")).toBeInTheDocument();
    expect(within(card).getByText(/shared with regional-transport/)).toBeInTheDocument();
    expect(within(card).getByText("ngsi-ld")).toBeInTheDocument();
    expect(within(card).getByText("csv")).toBeInTheDocument();
    expect(within(card).getByText("lane yellow")).toBeInTheDocument();
    const hidden = within(card).getByText("maintenanceNote");
    expect(hidden.className).toContain("line-through");
    expect(within(card).getByText("BikeHireDockingStation")).toBeInTheDocument();
  });

  it("renders what the run stream carried as text, never as markup (AG-46)", async () => {
    renderCard();
    expect(await screen.findByText("<b>internalCode</b>")).toBeInTheDocument();
    expect(document.querySelector("b")).toBeNull();
  });

  it("leaves the prefill for the endpoints page and opens it", async () => {
    const router = renderCard();
    await screen.findByRole("region", { name: en.agentRun.proposal.title });
    await userEvent.click(screen.getByRole("button", { name: en.agentRun.proposal.review }));
    await vi.waitFor(() => {
      expect(router.state.location.pathname).toBe("/projects/helsinki/endpoints");
    });
    const prefill = takePrefill("/projects/helsinki/endpoints") as Record<string, unknown> | null;
    expect(prefill?.name).toBe("bikes-regional-transport");
    expect(prefill?.slug).toBe(SLUG);
    expect(prefill?.hiddenAttributes).toEqual(["maintenanceNote", "<b>internalCode</b>"]);
  });

  it("copies a draft policy as text", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    renderCard();
    const card = await screen.findByRole("region", { name: en.agentRun.proposal.title });
    await userEvent.click(within(card).getByText("Draft policies (1)"));
    await userEvent.click(within(card).getByRole("button", { name: en.agentRun.proposal.copyPolicy }));
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(JSON.parse(writeText.mock.calls[0][0] as string).spec.operations).toEqual(["retrieveOps"]);
  });

  it("takes only a proposal the Portal wrote", () => {
    expect(proposalOf(PROPOSAL)?.slug).toBe(SLUG);
    expect(proposalOf({ slug: SLUG })).toBeNull();
    expect(proposalOf("x")).toBeNull();
    expect(proposalOf(null)).toBeNull();
  });

  // T-3986: the app builder draws this card only in a model run, which the live walk cannot start (spend guard).
  it("has_no_serious_axe_violation_as_a_model_run_draws_it", async () => {
    renderCard();
    await screen.findByRole("region", { name: en.agentRun.proposal.title });
    await expectNoAxeViolations(document.body);
  });
});
