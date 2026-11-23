/** JobsMenu: opening a job's page, the minute tick and a finished job being dismissed (T-7585). */
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { finishJob, resetJobs, startJob, useJobs } from "../src/jobs";

const navigate = vi.fn();
vi.mock("@tanstack/react-router", () => ({ useNavigate: () => navigate }));
const { JobsMenu } = await import("../src/components/JobsMenu");

function reply(body: unknown): Response {
  return new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } });
}

function serve() {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const path = new URL((input as Request).url).pathname;
      if (path.endsWith("/knowledge/sources")) return reply({ items: [{ source: "city web", job: { state: "running", attempts: 1, error: null } }] });
      return reply({ run: { number: 9, status: "in_progress", startedAt: "2026-10-07T10:00:00Z" }, typicalSeconds: null });
    }),
  );
}

function renderMenu() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <JobsMenu />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  localStorage.clear();
  resetJobs();
  navigate.mockReset();
  serve();
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  localStorage.clear();
  resetJobs();
});

describe("the jobs menu", () => {
  it("goes to the App's page and to the knowledge source's page, with the names escaped", async () => {
    const user = userEvent.setup();
    startJob({ kind: "appBuild", project: "helsinki", name: "bikes", after: 1 });
    startJob({ kind: "knowledgeCrawl", project: "helsinki", name: "city web", after: 0 });
    renderMenu();
    await user.click(await screen.findByRole("button"));
    const items = await screen.findAllByRole("menuitem");
    expect(items).toHaveLength(2);
    await user.click(items[0]);
    expect(navigate).toHaveBeenLastCalledWith({ href: expect.stringMatching(/^\/projects\/helsinki\/(apps\/bikes|knowledge\/city%20web)$/) });
    await user.click(await screen.findByRole("button"));
    await user.click((await screen.findAllByRole("menuitem"))[1]);
    const hrefs = navigate.mock.calls.map((call) => (call[0] as { href: string }).href).sort();
    expect(hrefs).toEqual(["/projects/helsinki/apps/bikes", "/projects/helsinki/knowledge/city%20web"]);
  });

  it("redraws on the half-minute tick and drops a finished job when it is opened", async () => {
    const intervals: (() => void)[] = [];
    const real = window.setInterval.bind(window);
    vi.spyOn(window, "setInterval").mockImplementation(((handler: TimerHandler, ms?: number) => {
      if (ms === 30_000 && typeof handler === "function") intervals.push(handler as () => void);
      return real(() => undefined, 1_000_000);
    }) as typeof window.setInterval);
    const user = userEvent.setup();
    startJob({ kind: "appBuild", project: "helsinki", name: "bikes", after: 1 });
    renderMenu();
    await screen.findByRole("button");
    expect(intervals.length).toBeGreaterThan(0);
    act(() => intervals.forEach((tick) => tick()));
    let ids: string[] = [];
    function Probe() {
      ids = useJobs().map((job) => job.id);
      return null;
    }
    render(<Probe />);
    act(() => finishJob(ids[0], "succeeded"));
    await user.click(await screen.findByRole("button"));
    await user.click(await screen.findByRole("menuitem"));
    expect(navigate).toHaveBeenCalledWith({ href: "/projects/helsinki/apps/bikes" });
    expect(ids).toEqual([]);
  });
});
