/**
 * T-1837: the indicator-pipeline card against the UI contract (UI-11, UI-16, UI-44, AG-74,
 * AG-46, PL-45, PL-51).
 *
 * `kpi_pipeline_card.test.tsx` keeps what the card reads out of a run's stream. What is asserted
 * here is the card as a control surface: it names itself as a region, the verdict is words and
 * not colour alone, the one action that writes anything says what it is doing and what was
 * refused, every string holds in four languages, every value out of the stream is text, and axe
 * is clean in each state.
 */
import { cleanup, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { KpiPipelineCard } from "../src/pages/apps/KpiPipelineCard";
import type { KpiPipeline } from "../src/pages/apps/KpiPipelineCard";
import {
  expectNoAxeViolations,
  inEveryLocale,
  json,
  problem,
  renderPage,
  tabOrder,
} from "./page_contract";

const PROJECT = "helsinki";

function pipeline(overrides: Partial<KpiPipeline> = {}): KpiPipeline {
  return {
    name: "pm10-daily",
    title: "PM10, daily average",
    formula: "avg(pm10) over 1d",
    trigger: "every day at 02:00",
    sourceEndpoint: "air-public",
    targetSpace: "indicators",
    verdict: { ok: true, findings: [] },
    drafts: [],
    ...overrides,
  };
}

const DRAFTS = [
  {
    kind: "ContextSpace",
    name: "indicators",
    plural: "spaces",
    manifest: { metadata: { name: "indicators" } },
  },
  {
    kind: "Endpoint",
    name: "indicators-public",
    plural: "endpoints",
    manifest: { metadata: { name: "indicators-public" } },
  },
];

function renderCard(value: KpiPipeline, refuse?: { status: number; detail: string }) {
  return renderPage(<KpiPipelineCard project={PROJECT} pipeline={value} />, {
    path: `/projects/${PROJECT}/apps/air`,
    answer: (_url, request) => {
      if (request.method !== "POST") return undefined;
      if (refuse) return problem(refuse.status, refuse.detail);
      return json({ metadata: { name: "chg-1" } });
    },
  });
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("the indicator pipeline card", () => {
  it("names itself and what the pipeline does", async () => {
    renderCard(pipeline());
    expect(
      await screen.findByRole("region", { name: en.agentRun.kpiPipeline.title }),
    ).toBeInTheDocument();
    expect(screen.getByText("PM10, daily average")).toBeInTheDocument();
    expect(screen.getByText("avg(pm10) over 1d")).toBeInTheDocument();
    expect(screen.getByText("air-public → indicators")).toBeInTheDocument();
  });

  it("falls back to the pipeline's name when it has no title", async () => {
    renderCard(pipeline({ title: undefined }));
    expect(await screen.findByText("pm10-daily")).toBeInTheDocument();
  });

  // The verdict is a sentence, not a green dot: the three outcomes read differently in words.
  it.each([
    [{ ok: true, findings: [] }, en.agentRun.kpiPipeline.tested],
    [
      { ok: false, untested: "no rows in the last day", findings: [] },
      en.agentRun.kpiPipeline.untested.replace("{reason}", "no rows in the last day"),
    ],
    [{ ok: false, findings: [] }, en.agentRun.kpiPipeline.failed],
  ] as const)("says the verdict in words", async (verdict, said) => {
    renderCard(pipeline({ verdict: { ...verdict, findings: [...verdict.findings] } }));
    expect(await screen.findByTestId("kpi-pipeline-verdict")).toHaveTextContent(said);
  });

  it("lists what the test found when it failed", async () => {
    renderCard(
      pipeline({ verdict: { ok: false, findings: ["pm10 is missing on 3 of 10 entities"] } }),
    );
    expect(await screen.findByText("pm10 is missing on 3 of 10 entities")).toBeInTheDocument();
  });

  // The one action that writes: it names every draft first, says while it is working, and says
  // how many landed.
  it("names the drafts before proposing them, and says how many landed", async () => {
    renderCard(pipeline({ drafts: DRAFTS }));
    expect(
      await screen.findByText(en.agentRun.kpiPipeline.newSpace.replace("{space}", "indicators")),
    ).toBeInTheDocument();
    expect(screen.getByText("ContextSpace indicators")).toBeInTheDocument();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: en.agentRun.kpiPipeline.propose }));
    expect(
      await screen.findByText(en.agentRun.kpiPipeline.proposed.replace("{count}", "2")),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: en.agentRun.kpiPipeline.propose }),
    ).not.toBeInTheDocument();
  });

  // UI-44: a refusal names the draft that was refused and carries the API's own sentence — and
  // it stops there, so nothing half of a space is proposed silently.
  it("says which draft was refused and why, and proposes no more", async () => {
    renderCard(pipeline({ drafts: DRAFTS }), {
      status: 403,
      detail: "You may not propose a context space here.",
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.agentRun.kpiPipeline.propose }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("ContextSpace indicators");
    expect(alert).toHaveTextContent("You may not propose a context space here.");
    expect(
      screen.queryByText(/Proposed \d+ manifests/),
    ).not.toBeInTheDocument();
  });

  it("offers no proposal when the indicator space already exists", async () => {
    renderCard(pipeline({ drafts: [] }));
    await screen.findByRole("region", { name: en.agentRun.kpiPipeline.title });
    expect(
      screen.queryByRole("button", { name: en.agentRun.kpiPipeline.propose }),
    ).not.toBeInTheDocument();
  });

  // UI-16: the keyboard reaches both the action and the way to the pipelines, and nothing takes
  // focus on arrival.
  it("is reachable by keyboard and takes no focus on arrival", async () => {
    const { container } = renderCard(pipeline({ drafts: DRAFTS }));
    await screen.findByRole("button", { name: en.agentRun.kpiPipeline.propose });
    expect(document.activeElement).toBe(document.body);
    const reached = await tabOrder(container);
    expect(reached).toContain(screen.getByRole("button", { name: en.agentRun.kpiPipeline.propose }));
    expect(reached).toContain(screen.getByRole("link", { name: en.agentRun.kpiPipeline.open }));
  });

  it("says the same things in every language", async () => {
    await inEveryLocale(async (locale) => {
      cleanup();
      renderCard(pipeline({ drafts: DRAFTS }));
      expect(
        await screen.findByRole("region", { name: i18n.t("agentRun.kpiPipeline.title") }),
        `the card has no name in ${locale}`,
      ).toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: i18n.t("agentRun.kpiPipeline.propose") }),
      ).toBeInTheDocument();
    });
  });

  // AG-46: every string on the card comes from the run's stream, so every one is drawn as text.
  it("renders a formula that arrived as markup as text", async () => {
    renderCard(pipeline({ formula: "<img src=x onerror=alert(1)>" }));
    expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("has no axe violations with drafts to propose", async () => {
    const { container } = renderCard(pipeline({ drafts: DRAFTS }));
    await screen.findByRole("button", { name: en.agentRun.kpiPipeline.propose });
    await expectNoAxeViolations(container);
  });

  it("has no axe violations with a failed verdict", async () => {
    const { container } = renderCard(
      pipeline({ verdict: { ok: false, findings: ["pm10 is missing"] } }),
    );
    await screen.findByText("pm10 is missing");
    await expectNoAxeViolations(container);
  });
});
