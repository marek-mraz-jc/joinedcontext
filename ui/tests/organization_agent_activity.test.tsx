/**
 * T-4851: an agent's Activity tab (AG-134…AG-136, API/01 §51). Runs with their tool calls,
 * refusals and the reason of each write, tokens and cost; filters sent to the server; the alerts
 * above the runs; Stop on a run in flight through the cancel route, behind a confirmation.
 */
import { cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AgentActivity } from "../src/pages/organization/AgentActivity";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const t = en.organization.agents.activity;

const RUN = {
  id: "run-7f3a",
  project: "helsinki",
  profile: "builder",
  title: "Air quality dashboard",
  startedBy: "ana@hel.fi",
  status: "building",
  createdAt: "2026-10-21T02:10:00Z",
  spaces: ["air"],
  tokens: 41000,
  costUsd: 0.0125,
  refusals: 1,
  writes: 1,
  mayStop: true,
  steps: [
    {
      at: "2026-10-21T02:10:04Z",
      tool: "jc_resource_list",
      target: "Endpoint",
      verdict: "ok",
      write: false,
      durationMs: 9,
    },
    {
      at: "2026-10-21T02:10:09Z",
      tool: "jc_endpoint_propose",
      target: "air",
      verdict: "refused",
      write: true,
      reason: "forbidden: missing role steward",
    },
    {
      at: "2026-10-21T02:11:00Z",
      tool: "jc_resource_propose",
      target: "air",
      verdict: "ok",
      write: true,
      reason: "the sensor moved",
    },
  ],
};

const DONE = {
  ...RUN,
  id: "run-old",
  title: "Older run",
  status: "published",
  mayStop: false,
  refusals: 0,
  writes: 0,
  steps: [],
  costUsd: undefined,
};

const ALERT = {
  kind: "spendShare",
  project: "helsinki",
  profile: "builder",
  runId: "run-7f3a",
  runTitle: "Air quality dashboard",
  count: 85,
  at: "2026-10-21T02:10:00Z",
};

interface Harness {
  pages?: unknown[];
  status?: number;
  run?: string;
}

function render({
  pages = [{ runs: [RUN, DONE], alerts: [ALERT] }],
  status = 200,
  run,
}: Harness) {
  const asked: URL[] = [];
  const cancels: URL[] = [];
  const result = renderPage(
    <AgentActivity
      scope={run ? { project: "helsinki", run } : { profile: "builder" }}
    />,
    {
      path: "/organization/agents",
      answer: async (url, request) => {
        if (url.pathname === "/api/v1/organization/agents/activity") {
          asked.push(url);
          if (status !== 200) {
            return json(
              {
                type: "about:blank",
                title: "Forbidden",
                status,
                detail: "Your roles read no project.",
              },
              status,
            );
          }
          const index = url.searchParams.get("cursor") ? 1 : 0;
          return json(pages[index] ?? { runs: [], alerts: [] });
        }
        if (
          url.pathname ===
            "/api/v1/projects/helsinki/agent-runs/run-7f3a/cancel" &&
          request.method === "POST"
        ) {
          cancels.push(url);
          return json({ ...RUN, status: "cancelled" });
        }
        return undefined;
      },
    },
  );
  return { ...result, asked, cancels };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("an agent's Activity tab", () => {
  it("shows each run with its tool calls, the refusal's problem, the write's reason, tokens and cost", async () => {
    const { container, asked } = render({});
    const runs = await screen.findByRole("list", { name: t.runs });
    const cards = within(runs)
      .getAllByRole("listitem")
      .filter((item) => item.parentElement === runs);
    expect(cards).toHaveLength(2);
    const first = cards[0];
    expect(
      within(first).getByRole("heading", { name: "Air quality dashboard" }),
    ).toBeInTheDocument();
    expect(within(first).getByText("ana@hel.fi")).toBeInTheDocument();
    expect(within(first).getByText("41,000")).toBeInTheDocument();
    expect(within(first).getByText("$0.0125")).toBeInTheDocument();
    expect(within(first).getByText("Refused: 1")).toBeInTheDocument();
    fireEvent.click(within(first).getByText("Tool calls (3)"));
    expect(
      within(first).getByText(
        "Why it was refused: forbidden: missing role steward",
      ),
    ).toBeInTheDocument();
    expect(
      within(first).getByText("Reason: the sensor moved"),
    ).toBeInTheDocument();
    expect(within(cards[1]).getByText(t.costUnreported)).toBeInTheDocument();
    expect(within(cards[1]).queryByRole("button", { name: t.stop })).toBeNull();
    expect(asked[0].searchParams.get("profile")).toBe("builder");
    expect(asked[0].searchParams.has("project")).toBe(false);
    await expectNoAxeViolations(container);
  });

  it("shows the alerts above the runs", async () => {
    render({});
    const alerts = await screen.findByRole("list", { name: t.alerts });
    expect(
      within(alerts).getByText(
        "Run “Air quality dashboard” of builder has used 85 % of its token cap.",
      ),
    ).toBeInTheDocument();
  });

  it("sends the filters to the server and says when none match", async () => {
    const { asked } = render({
      pages: [
        {
          runs: [RUN],
          alerts: [],
          choices: { people: ["ana@hel.fi"], spaces: ["air"] },
        },
      ],
    });
    await screen.findByRole("list", { name: t.runs });
    // Only the values the server offers, so nobody types a name that reads as empty history.
    expect(
      within(screen.getByLabelText(t.person))
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual([en.organization.agents.activity.personAny, "ana@hel.fi"]);
    fireEvent.change(screen.getByLabelText(t.person), {
      target: { value: "ana@hel.fi" },
    });
    fireEvent.change(screen.getByLabelText(t.verdict), {
      target: { value: "refused" },
    });
    fireEvent.change(screen.getByLabelText(t.space), {
      target: { value: "air" },
    });
    fireEvent.click(screen.getByRole("button", { name: t.apply }));
    await vi.waitFor(() => expect(asked.length).toBeGreaterThan(1));
    const last = asked[asked.length - 1];
    expect(last.searchParams.get("person")).toBe("ana@hel.fi");
    expect(last.searchParams.get("verdict")).toBe("refused");
    expect(last.searchParams.get("space")).toBe("air");
    expect(screen.getByRole("button", { name: t.clear })).toBeInTheDocument();
  });

  it("pages older runs with the cursor", async () => {
    const { asked } = render({
      pages: [
        {
          runs: [RUN],
          alerts: [],
          nextCursor: "2026-10-21T02:10:00Z|run-7f3a",
        },
        { runs: [DONE], alerts: [] },
      ],
    });
    fireEvent.click(await screen.findByRole("button", { name: t.more }));
    await screen.findByRole("heading", { name: "Older run" });
    expect(asked[asked.length - 1].searchParams.get("cursor")).toBe(
      "2026-10-21T02:10:00Z|run-7f3a",
    );
    expect(screen.queryByRole("button", { name: t.more })).toBeNull();
  });

  it("stops a run in flight only after the person confirms", async () => {
    const { cancels } = render({
      run: "run-7f3a",
      pages: [{ runs: [RUN], alerts: [] }],
    });
    fireEvent.click(await screen.findByRole("button", { name: t.stop }));
    const dialog = await screen.findByRole("alertdialog");
    expect(
      within(dialog).getByText("Stop the run Air quality dashboard?"),
    ).toBeInTheDocument();
    expect(cancels).toHaveLength(0);
    fireEvent.click(
      within(dialog).getByRole("button", { name: t.stopConfirm }),
    );
    await vi.waitFor(() => expect(cancels).toHaveLength(1));
    // One run's tab has no filters to show.
    expect(screen.queryByRole("form", { name: t.filters })).toBeNull();
  });

  it("says who can grant read when the person reads no project, and has an empty state", async () => {
    render({ status: 403 });
    expect(
      await screen.findByText("Your roles read no project."),
    ).toBeInTheDocument();
    cleanup();
    render({ pages: [{ runs: [], alerts: [] }] });
    expect(await screen.findByText(t.none)).toBeInTheDocument();
  });
});

describe("alerts of an agent as notices (T-5301)", () => {
  const a = en.alerts;
  function alerting(alertProjects: string[]) {
    const puts: unknown[] = [];
    renderPage(<AgentActivity scope={{ profile: "builder" }} />, {
      path: "/organization/agents",
      answer: async (url, request) => {
        if (url.pathname === "/api/v1/organization/agents/activity") {
          return json({ runs: [RUN], alerts: [], choices: { people: [], spaces: [], alertProjects } });
        }
        if (url.pathname === "/api/v1/alerts" && request.method === "GET") return json({ items: [] });
        if (url.pathname === "/api/v1/alerts" && request.method === "PUT") {
          const body = await request.clone().json();
          puts.push(body);
          return json({ id: 9, mutedUntil: null, ...body });
        }
        return undefined;
      },
    });
    return puts;
  }

  it("an approver subscribes in a project the server offered, to the agent's own events", async () => {
    const puts = alerting(["espoo", "helsinki"]);
    fireEvent.click(await screen.findByRole("button", { name: t.alertMe }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(a.projectHelp)).toBeInTheDocument();
    fireEvent.change(within(dialog).getByLabelText(a.project), { target: { value: "helsinki" } });
    expect(within(dialog).getByLabelText(a.event.refusalSpike)).toBeChecked();
    expect(within(dialog).queryByLabelText(a.event.failure)).toBeNull();
    fireEvent.click(within(dialog).getByLabelText(a.event.offHoursWrite));
    fireEvent.click(within(dialog).getByRole("button", { name: a.subscribe }));
    await vi.waitFor(() => expect(puts).toHaveLength(1));
    expect(puts[0]).toEqual({
      project: "helsinki",
      scope: "agent",
      target: "builder",
      events: ["refusalSpike", "spendShare"],
      delivery: "portal",
    });
    await expectNoAxeViolations(document.body);
  });

  it("someone who approves nowhere is told why they cannot be alerted", async () => {
    alerting([]);
    const button = await screen.findByRole("button", { name: t.alertMe });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(screen.getAllByText(t.alertMeNone).length).toBeGreaterThan(0);
    expect(screen.queryByText(t.alertMeHint)).toBeNull();
  });
});
