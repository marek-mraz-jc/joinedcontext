/**
 * T-4165 (photo audit T-4163, pattern failed-read): a read that fails leaves the page in the shell
 * with one plain sentence and Retry. It used to tell a wrong story instead: the first-run checklist
 * at "0 of 5 done", "no create form here", a bare "Internal Server Error" without the page's
 * heading, or a page without its navigation.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import { queryKeys } from "../src/api/client";
import { jsonResponse, list, renderRoute } from "./pageHarness";

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

  // T-6950: both hosts of a model's source (its page and the space's Data model tab) say a refused
  // source read as a refusal: the server's words, no Retry, never "the Portal did not answer".
  it.each([["/projects/helsinki/models/air"], ["/projects/helsinki/spaces/ilma?tab=model"], ["/projects/helsinki/spaces/ilma?tab=data"]])(
    "says a refused model source on %s as a refusal",
    async (page) => {
      const model = {
        apiVersion: "joinedcontext.com/v1alpha1",
        kind: "DataModel",
        metadata: { name: "air", namespace: "helsinki" },
        spec: { contextSpaceRef: "ilma", version: "1.0.0" },
      };
      await renderRoute({
        path: page,
        answer: (path) =>
          path.endsWith("/datamodels/air/source")
            ? new Response(JSON.stringify({ type: "about:blank", title: "Forbidden", status: 403, detail: "answered 403" }), {
                status: 403,
                headers: { "Content-Type": "application/problem+json", "X-Request-Id": "req-403" },
              })
            : path.endsWith("/datamodels/air")
              ? jsonResponse(model)
              : path.endsWith("/datamodels")
                ? jsonResponse(list([model]))
                : path.endsWith("/spaces/ilma")
                  ? jsonResponse({ ...model, kind: "ContextSpace", metadata: { name: "ilma", namespace: "helsinki" }, spec: {} })
                  : undefined,
      });
      expect(await screen.findByText("answered 403")).toBeInTheDocument();
      expect(screen.queryByText(en.app.error.generic)).not.toBeInTheDocument();
      expect(screen.queryByRole("button", { name: en.app.error.retry })).not.toBeInTheDocument();
      // An unread source says nothing about the model's types (UI judge, T-6950).
      expect(screen.queryByText(en.spaces.inside.noTypes.replace("{model}", "air"))).not.toBeInTheDocument();
      expect(screen.queryByText(en.spaces.inside.dataNoTypes)).not.toBeInTheDocument();
    },
  );

  // T-6950 (integrator UI judge): the model page keeps what the record knows beside an unread
  // source, and Edit model says why it cannot open instead of a faded primary that does nothing.
  it("keeps the model's record and explains Edit model when the source is refused", async () => {
    const model = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataModel",
      metadata: { name: "air", namespace: "helsinki" },
      spec: { contextSpaceRef: "ilma", version: "1.2.0", lifecycle: "published" },
    };
    await renderRoute({
      path: "/projects/helsinki/models/air",
      answer: (path) =>
        path.endsWith("/datamodels/air/source")
          ? new Response(JSON.stringify({ type: "about:blank", title: "Forbidden", status: 403, detail: "answered 403" }), {
              status: 403,
              headers: { "Content-Type": "application/problem+json" },
            })
          : path.endsWith("/datamodels")
            ? jsonResponse(list([model]))
            : undefined,
    });
    expect(await screen.findByText("answered 403")).toBeInTheDocument();
    const known = screen.getByRole("region", { name: en.models.page.known });
    expect(within(known).getByText("1.2.0")).toBeInTheDocument();
    expect(within(known).getByText(en.models.lifecycleOption.published)).toBeInTheDocument();
    expect(within(known).getByRole("link", { name: "ilma" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: en.models.page.edit })).toHaveAccessibleDescription(en.models.page.editUnread);
    // A touch screen has no tooltip: the reason is on the page as well.
    expect(within(known).getByText(en.models.page.editUnread)).toBeInTheDocument();
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
