/**
 * An agent's Activity tab: the date filter reaches the server, Clear puts every filter back,
 * Retry asks a failed read again, and the alert dialog closes without subscribing.
 */
import { cleanup, fireEvent, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AgentActivity } from "../src/pages/organization/AgentActivity";
import { json, problem, renderPage } from "./page_contract";

const t = en.organization.agents.activity;

const RUN = {
  id: "run-7f3a",
  project: "helsinki",
  profile: "builder",
  title: "Air quality dashboard",
  startedBy: "ana@hel.fi",
  status: "published",
  createdAt: "2026-10-21T02:10:00Z",
  spaces: ["air"],
  tokens: 100,
  costUsd: 0.01,
  refusals: 0,
  writes: 0,
  mayStop: false,
  steps: [],
};

const PAGE = { runs: [RUN], alerts: [], choices: { people: ["ana@hel.fi"], spaces: ["air"], alertProjects: ["helsinki"] } };

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderActivity(answer: (url: URL, request: Request) => Response | undefined) {
  const asked: URL[] = [];
  const puts: string[] = [];
  renderPage(<AgentActivity scope={{ profile: "builder" }} />, {
    path: "/organization/agents",
    answer: (url, request) => {
      if (url.pathname === "/api/v1/organization/agents/activity") {
        asked.push(url);
        return answer(url, request);
      }
      if (url.pathname === "/api/v1/alerts") {
        if (request.method === "PUT") puts.push(request.method);
        return json({ items: [] });
      }
      return undefined;
    },
  });
  return { asked, puts };
}

describe("an agent's Activity tab: filters and failures", () => {
  it("sends the date as the start of that day and Clear drops every filter", async () => {
    const { asked } = renderActivity(() => json(PAGE));
    await screen.findByRole("list", { name: t.runs });
    expect(screen.queryByRole("button", { name: t.clear })).toBeNull();
    fireEvent.change(screen.getByLabelText(t.since), { target: { value: "2026-10-01" } });
    fireEvent.change(screen.getByLabelText(t.person), { target: { value: "ana@hel.fi" } });
    fireEvent.click(screen.getByRole("button", { name: t.apply }));
    await vi.waitFor(() => expect(asked.length).toBeGreaterThan(1));
    const filtered = asked[asked.length - 1];
    expect(filtered.searchParams.get("since")).toBe(new Date("2026-10-01T00:00:00").toISOString());
    expect(filtered.searchParams.get("person")).toBe("ana@hel.fi");

    fireEvent.click(screen.getByRole("button", { name: t.clear }));
    await vi.waitFor(() => expect(screen.queryByRole("button", { name: t.clear })).toBeNull());
    await vi.waitFor(() => {
      const last = asked[asked.length - 1];
      expect(last.searchParams.has("since")).toBe(false);
      expect(last.searchParams.has("person")).toBe(false);
    });
    expect(screen.getByLabelText(t.since)).toHaveValue("");
    expect(screen.getByLabelText(t.person)).toHaveValue("");
  });

  it("says a filtered list found nothing, differently from a list that has no run", async () => {
    renderActivity((url) => json(url.searchParams.has("verdict") ? { ...PAGE, runs: [] } : PAGE));
    await screen.findByRole("list", { name: t.runs });
    fireEvent.change(screen.getByLabelText(t.verdict), { target: { value: "refused" } });
    fireEvent.click(screen.getByRole("button", { name: t.apply }));
    expect(await screen.findByText(t.noneFiltered)).toBeInTheDocument();
  });

  it("asks a failed read again on Retry", async () => {
    let healthy = false;
    const { asked } = renderActivity(() => (healthy ? json(PAGE) : problem(503, "The activity log is restarting.")));
    expect(await screen.findByText(/The activity log is restarting/)).toBeInTheDocument();
    healthy = true;
    await userEvent.setup().click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByRole("heading", { name: "Air quality dashboard" })).toBeInTheDocument();
    expect(asked).toHaveLength(2);
  });

  it("closes the alert dialog on Escape and subscribes to nothing", async () => {
    const { puts } = renderActivity(() => json(PAGE));
    fireEvent.click(await screen.findByRole("button", { name: t.alertMe }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("button", { name: en.alerts.subscribe })).toBeInTheDocument();
    await userEvent.setup().keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("button", { name: t.alertMe })).toBeInTheDocument();
    expect(puts).toEqual([]);
  });
});
