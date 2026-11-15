/**
 * T-4165 (photo audit T-4163, pattern failed-read): a read that fails leaves the page in the shell
 * with one plain sentence and Retry. It used to tell a wrong story instead: the first-run checklist
 * at "0 of 5 done", "no create form here", a bare "Internal Server Error" without the page's
 * heading, or a page without its navigation.
 */
import { cleanup, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import { queryKeys } from "../src/api/client";
import { renderRoute } from "./pageHarness";

const failure = () => new Response(null, { status: 500, statusText: "Internal Server Error" });

afterEach(() => {
  vi.unstubAllGlobals();
  window.localStorage.clear();
});

describe("a failed read", () => {
  it("keeps Home from counting a project it could not read", async () => {
    await renderRoute({
      path: "/projects/helsinki/home",
      permissions: { project: "helsinki", bootstrap: true, grants: [] },
      answer: (path) => (/\/(spaces|datasources|pipelines|endpoints)$/.test(path) ? failure() : undefined),
    });
    expect(await screen.findByText(en.app.error.server)).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: en.home.firstRun.title })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
  });

  it("keeps the Applications heading and says what failed in words", async () => {
    await renderRoute({
      path: "/projects/helsinki/apps",
      answer: (path) => (path.endsWith("/apps") ? failure() : undefined),
    });
    expect(await screen.findByText(en.app.error.server)).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: en.apps.title })).toBeInTheDocument();
    expect(screen.queryByText("Internal Server Error")).not.toBeInTheDocument();
  });

  it("keeps the shell on the Organization page while the project list fails", async () => {
    window.localStorage.setItem("jc.lastProject", "helsinki");
    const rendered = await renderRoute({ path: "/organization/endpoints", projects: failure });
    expect(await screen.findByText(en.app.error.server)).toBeInTheDocument();
    // The shell's own project switcher does not set the list asking again in a loop.
    await new Promise((resolve) => setTimeout(resolve, 500));
    expect(rendered.calls().filter((call) => call === "GET /api/v1/projects").length).toBeLessThan(4);
    // The project navigation and the person's menu stay, so they can go elsewhere.
    expect(screen.getAllByRole("navigation").length).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: en.nav.projects })).toBeInTheDocument();
  });

  // A-8-07: the Dashboards and Organization endpoints pages say a failure through PageFailed: a
  // server failure is quoted with Retry, a refusal gets no Retry that cannot help.
  it.each([
    ["/projects/helsinki/dashboards", (path: string) => path.endsWith("/dashboards")],
    ["/organization/endpoints", (path: string) => path.endsWith("/endpoints")],
  ])("says a failure on %s the shared way", async (page, failing) => {
    const problem = (status: number) =>
      new Response(JSON.stringify({ type: "about:blank", title: "x", status, detail: `answered ${status}` }), {
        status,
        headers: { "Content-Type": "application/problem+json", "X-Request-Id": `req-${status}` },
      });
    await renderRoute({ path: page, answer: (path) => (failing(path) ? problem(500) : undefined) });
    expect(await screen.findByText("answered 500")).toBeInTheDocument();
    expect(screen.getByText(/req-500/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
    cleanup();
    await renderRoute({ path: page, answer: (path) => (failing(path) ? problem(403) : undefined) });
    expect(await screen.findByText("answered 403")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.app.error.retry })).not.toBeInTheDocument();
  });

  it("is not told as a missing create form", async () => {
    await renderRoute({
      path: "/projects/helsinki/datasources/new",
      answer: (path) => (path.startsWith("/api/v1/projects/helsinki/") ? failure() : undefined),
    });
    expect(await screen.findByText(en.app.error.server)).toBeInTheDocument();
    // The verdict is taken after the reads settle; give it the commit it waits for.
    await waitFor(() => expect(screen.queryByText(en.form.notOpenNew)).not.toBeInTheDocument());
    expect(screen.queryByText(en.form.opening)).not.toBeInTheDocument();
  });
});

describe("an address that leads nowhere (T-4165, photo audit T-4157)", () => {
  it("keeps the shell for a signed-in person, so the navigation is the way on", async () => {
    window.localStorage.setItem("jc.lastProject", "helsinki");
    await renderRoute({ path: "/no-such-page" });
    expect(await screen.findByRole("heading", { level: 1, name: en.app.notFound.title })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.nav.projects })).toBeInTheDocument();
  });

  it("says the page does not exist, not that a project is closed, when the project last worked in is gone", async () => {
    // jc-multiorg-portal's frameFor turned a stored project the list no longer names into the
    // project refusal, for an address that needs no project at all.
    window.localStorage.setItem("jc.lastProject", "gone");
    const { client } = await renderRoute({ path: "/no-such-page" });
    // Only once the list is in does the frame know "gone" is not among it.
    await waitFor(() => expect(client.getQueryState(queryKeys.projects())?.status).toBe("success"));
    expect(await screen.findByRole("heading", { level: 1, name: en.app.notFound.title })).toBeInTheDocument();
    expect(screen.getByText(en.app.notFound.description.replace("{path}", "/no-such-page"))).toBeInTheDocument();
    expect(screen.queryByText(en.form.projectHidden)).toBeNull();
  });
});
