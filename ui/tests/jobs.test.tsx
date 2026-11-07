/**
 * T-3245: what a person started keeps going while they work elsewhere. The jobs store keeps it in
 * this browser; the Shell's menu says how far it is, the estimate when there is one, says when it
 * finished, and leads back to its page.
 */
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { progressOf, resetJobs, startJob } from "../src/jobs";

vi.mock("@tanstack/react-router", () => ({ useNavigate: () => vi.fn() }));
const { JobsMenu } = await import("../src/components/JobsMenu");

const BUILD = "/api/v1/projects/helsinki/apps/bikes/build";

function serve(build: unknown) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin));
      const path = new URL(request.url).pathname;
      if (path === BUILD) return new Response(JSON.stringify(build), { status: 200, headers: { "Content-Type": "application/json" } });
      if (path.endsWith("/knowledge/sources")) {
        return new Response(JSON.stringify({ items: [{ source: "city-web", job: { state: "failed", attempts: 1, error: null } }] }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      return new Response("{}", { status: 404 });
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

const run = (over: Record<string, unknown>) => ({ status: "completed", conclusion: "success", commit: "c", url: "u", ...over });

describe("the person's jobs (T-3245)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
    resetJobs();
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
    resetJobs();
  });

  it("starting the same thing again replaces it, and a broken store is an empty list", () => {
    startJob({ kind: "appBuild", project: "helsinki", name: "bikes", after: 7 });
    startJob({ kind: "appBuild", project: "helsinki", name: "bikes", after: 8 });
    const stored = JSON.parse(localStorage.getItem("jc.jobs") ?? "[]") as { after: number }[];
    expect(stored.map((job) => job.after)).toEqual([8]);

    localStorage.setItem("jc.jobs", "{not json");
    resetJobs();
    const { container } = renderMenu();
    expect(container.querySelector("button")).toBeNull();
  });

  it("the estimate is the last success's length, and none without one", () => {
    const now = Date.parse("2026-10-07T10:02:00Z");
    expect(progressOf("2026-10-07T10:00:00Z", 185, now)).toEqual({ elapsedMinutes: 2, leftMinutes: 2 });
    expect(progressOf("2026-10-07T10:00:00Z", 60, now)).toEqual({ elapsedMinutes: 2, leftMinutes: 0 });
    expect(progressOf("2026-10-07T10:00:00Z", null, now)).toEqual({ elapsedMinutes: 2, leftMinutes: undefined });
  });

  it("the run that was newest when Rebuild was pressed is not the job's: it waits", async () => {
    startJob({ kind: "appBuild", project: "helsinki", name: "bikes", after: 7 });
    serve({ run: run({ number: 7 }), typicalSeconds: 185, rebuild: { allowed: true } });
    renderMenu();
    const button = await screen.findByRole("button", { name: /What you started: 1 running/ });
    await userEvent.click(button);
    expect(await screen.findByText(en.jobs.queued)).toBeInTheDocument();
    expect(screen.getByText("Build of bikes")).toBeInTheDocument();
  });

  it("a running build shows its time against the estimate", async () => {
    startJob({ kind: "appBuild", project: "helsinki", name: "bikes", after: 7 });
    serve({
      run: run({ number: 8, status: "in_progress", conclusion: undefined, startedAt: new Date(Date.now() - 61_000).toISOString() }),
      typicalSeconds: 185,
      rebuild: { allowed: true },
    });
    renderMenu();
    await userEvent.click(await screen.findByRole("button", { name: /What you started/ }));
    expect(await screen.findByText("Running for 1 minute, about 3 minutes left")).toBeInTheDocument();
  });

  it("a finished build is said once and stays listed with how it ended, after the page that started it is gone", async () => {
    startJob({ kind: "appBuild", project: "helsinki", name: "bikes", after: 7 });
    serve({ run: run({ number: 8 }), typicalSeconds: 185, rebuild: { allowed: true } });
    renderMenu();
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("The build of bikes finished."));
    const button = screen.getByRole("button", { name: "What you started: nothing running, 1 finished" });
    await userEvent.click(button);
    expect(await screen.findByText(en.jobs.outcome.succeeded)).toBeInTheDocument();
    const stored = JSON.parse(localStorage.getItem("jc.jobs") ?? "[]") as { outcome: string; seen: boolean }[];
    expect(stored[0]).toMatchObject({ outcome: "succeeded", seen: true });
  });

  it("a failed crawl says so and points at its page for why", async () => {
    startJob({ kind: "knowledgeCrawl", project: "helsinki", name: "city-web" });
    serve({});
    renderMenu();
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("The crawl of city-web failed."));
    await userEvent.click(screen.getByRole("button", { name: /What you started/ }));
    expect(await screen.findByText(en.jobs.outcome.failed)).toBeInTheDocument();
  });

  it("two building sources of one project are one list read, not one each (T-6230)", async () => {
    startJob({ kind: "knowledgeCrawl", project: "helsinki", name: "city-web" });
    startJob({ kind: "knowledgeCrawl", project: "helsinki", name: "other-web" });
    serve({});
    const fetched = vi.mocked(globalThis.fetch);
    renderMenu();
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("The crawl of city-web failed."));
    const reads = fetched.mock.calls.filter(([input]) => String(input instanceof Request ? input.url : input).includes("/knowledge/sources"));
    expect(reads).toHaveLength(1);
  });

  it("another tab's job appears here", async () => {
    serve({ run: run({ number: 7 }), rebuild: { allowed: true } });
    const { container } = renderMenu();
    expect(container.querySelector("button")).toBeNull();
    act(() => {
      localStorage.setItem(
        "jc.jobs",
        JSON.stringify([{ id: "appBuild:helsinki:bikes", kind: "appBuild", project: "helsinki", name: "bikes", after: 7, startedAt: new Date().toISOString() }]),
      );
      window.dispatchEvent(new StorageEvent("storage", { key: "jc.jobs" }));
    });
    expect(await screen.findByRole("button", { name: /1 running/ })).toBeInTheDocument();
  });
});
