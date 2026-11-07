/**
 * Where a published application is built, on its App page (T-2609, AP-103, ADR-N-028): the
 * repository, the newest run and the package, each opening in the forge, and Rebuild, which is
 * offered to a person who may propose an App and says why it is not otherwise.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { AppBuildPanel } from "../src/pages/apps/AppBuildPanel";
import en from "../src/locales/en.json";
import { expectNoViolations } from "./checks";

const BUILD = "/api/v1/projects/helsinki/apps/bikes/build";
const REBUILD = "/api/v1/projects/helsinki/apps/bikes/rebuild";
const BUILDS = "/api/v1/projects/helsinki/apps/bikes/builds";
const RESTORE = "/api/v1/projects/helsinki/apps/bikes/restore";
const FORGE = "https://forge.example/user/login?redirect_to=";

function built(overrides: Record<string, unknown> = {}) {
  return {
    repositoryUrl: `${FORGE}%2Fjoinedcontext%2Fhelsinki_bikes`,
    run: {
      status: "completed",
      conclusion: "success",
      commit: "3f1c0e2d7a6b5c4f1b9c0e2d7a6b5c4f1b9c0e2d",
      url: `${FORGE}%2Fjoinedcontext%2Fhelsinki_bikes%2Factions%2Fruns%2F7`,
    },
    packageUrl: `${FORGE}%2Fjoinedcontext%2F-%2Fpackages%2Fgeneric%2Fapp-bikes%2F3f1c0e2`,
    rebuild: { allowed: true },
    ...overrides,
  };
}

type Reply = { status: number; body: unknown };

function renderPanel(answer: Reply, rebuild: Reply = { status: 202, body: {} }, routes: Record<string, Reply> = {}) {
  const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
    const request = typeof input === "string" || input instanceof URL ? null : input;
    const url = new URL(request ? request.url : String(input), window.location.origin);
    const reply = ({ status, body }: { status: number; body: unknown }) =>
      new Response(status === 202 ? null : JSON.stringify(body), {
        status,
        headers: { "Content-Type": status < 300 ? "application/json" : "application/problem+json" },
      });
    if (url.pathname === BUILD) return reply(answer);
    if (url.pathname === REBUILD && request?.method === "POST") return reply(rebuild);
    const routed = routes[`${request?.method ?? "GET"} ${url.pathname}`];
    if (routed) return reply(routed);
    return reply({ status: 404, body: { title: "Not Found", status: 404 } });
  });
  vi.stubGlobal("fetch", fetchMock);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <AppBuildPanel project="helsinki" name="bikes" />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { fetchMock, view };
}

async function panel(): Promise<HTMLElement> {
  const heading = await screen.findByRole("heading", { name: en.apps.build.title });
  return heading.closest("section") as HTMLElement;
}

describe("the build of an application on the forge", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  // AP-103, PF-81: three links out to the forge, each the address the API gave, behind its sign-in.
  it("links the repository, the latest run and the package, and says the run built its commit", async () => {
    renderPanel({ status: 200, body: built() });
    const section = await panel();

    expect(within(section).getByText(en.apps.build.run.succeeded.replace("{commit}", "3f1c0e2"))).toBeInTheDocument();
    const href = (name: string) => within(section).getByRole("link", { name: new RegExp(name) }).getAttribute("href");
    expect(href(en.apps.build.repository)).toBe(`${FORGE}%2Fjoinedcontext%2Fhelsinki_bikes`);
    expect(href(en.apps.build.runLink)).toContain("actions%2Fruns%2F7");
    expect(href(en.apps.build.package)).toContain("app-bikes");
    await expectNoViolations(section);
  });

  // AP-86: a run in progress reads as building; a failed one points at its log.
  it("reads a run in progress as building and a failed run as failed", async () => {
    const { view } = renderPanel({ status: 200, body: built({ run: { ...built().run, status: "in_progress", conclusion: undefined } }) });
    expect(within(await panel()).getByText(en.apps.build.run.building.replace("{commit}", "3f1c0e2"))).toBeInTheDocument();
    view.unmount();
    vi.unstubAllGlobals();

    renderPanel({ status: 200, body: built({ run: { ...built().run, conclusion: "failure" }, packageUrl: null }) });
    const section = await panel();
    expect(within(section).getByText(en.apps.build.run.failed.replace("{commit}", "3f1c0e2"))).toBeInTheDocument();
    expect(within(section).queryByRole("link", { name: new RegExp(en.apps.build.package) })).toBeNull();
  });

  // AP-103: Rebuild asks the forge once, and says it did.
  it("dispatches a rebuild once and says the forge was asked", async () => {
    const user = userEvent.setup();
    const { fetchMock } = renderPanel({ status: 200, body: built() });
    await user.click(within(await panel()).getByRole("button", { name: en.apps.build.rebuild }));

    expect(await screen.findByText(en.apps.build.started)).toBeInTheDocument();
    const posts = fetchMock.mock.calls
      .map(([input]) => input as Request)
      .filter((request) => typeof request !== "string" && request.method === "POST");
    expect(posts.map((request) => new URL(request.url).pathname)).toEqual([REBUILD]);
    // T-3245: the build is now one of the person's jobs, after run 7, which the Shell watches.
    const jobs = JSON.parse(localStorage.getItem("jc.jobs") ?? "[]") as { id: string; after: number }[];
    expect(jobs.map((job) => [job.id, job.after])).toEqual([["appBuild:helsinki:bikes", 0]]);
    localStorage.removeItem("jc.jobs");
  });

  // UI-44: a person who may not propose an App reaches the button and is told why; nothing is sent.
  it("keeps Rebuild reachable but refused, with the reason, for a reader", async () => {
    const user = userEvent.setup();
    const reason = "Rebuild needs propose on App in project helsinki";
    const { fetchMock } = renderPanel({ status: 200, body: built({ rebuild: { allowed: false, reason } }) });
    const button = within(await panel()).getByRole("button", { name: en.apps.build.rebuild });

    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText(reason)).toBeInTheDocument();
    await user.click(button);
    expect(fetchMock.mock.calls.some(([input]) => (input as Request).method === "POST")).toBe(false);
  });

  // AP-103: the forge's refusal is shown in its own words.
  it("shows the forge's reason when the dispatch is refused", async () => {
    const user = userEvent.setup();
    const detail = "the forge did not start the build of 'bikes': Actions are disabled for this repository";
    renderPanel({ status: 200, body: built() }, { status: 503, body: { title: "Service Unavailable", status: 503, detail } });
    await user.click(within(await panel()).getByRole("button", { name: en.apps.build.rebuild }));
    expect(await screen.findByText(detail)).toBeInTheDocument();
  });

  // AP-171: the earlier builds are listed with Restore; the current one is not offered. Restore
  // opens the merge request for exactly that commit and links it; the data note is always there.
  it("lists earlier builds and restores one as a merge request", async () => {
    const user = userEvent.setup();
    const old = "9a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b";
    const pullRequestUrl = "https://forge.example/joinedcontext/helsinki_bikes/pulls/3";
    const { fetchMock } = renderPanel({ status: 200, body: built() }, undefined, {
      [`GET ${BUILDS}`]: {
        status: 200,
        body: {
          builds: [
            { commit: built().run.commit, number: 8, current: true },
            { commit: old, number: 7, completedAt: "2026-10-05T08:12:40Z", current: false },
          ],
          restore: { allowed: true },
          dataNote: "",
        },
      },
      [`POST ${RESTORE}`]: { status: 201, body: { branch: "restore/9a2b3c4-1", pullRequestUrl } },
    });
    const section = await panel();
    expect(await within(section).findByText(en.apps.build.history.dataNote)).toBeInTheDocument();
    const restore = (commit: string) => en.apps.build.history.restoreOne.replace("{commit}", commit);
    expect(within(section).queryByRole("button", { name: restore("3f1c0e2") })).toBeNull();
    await user.click(within(section).getByRole("button", { name: restore("9a2b3c4") }));

    const link = await within(section).findByRole("link", { name: new RegExp(en.apps.build.history.request) });
    expect(link.getAttribute("href")).toBe(pullRequestUrl);
    const posted = fetchMock.mock.calls
      .map(([input]) => input as Request)
      .filter((request) => typeof request !== "string" && request.method === "POST");
    expect(posted.map((request) => new URL(request.url).pathname)).toEqual([RESTORE]);
    expect(await posted[0].json()).toEqual({ commit: old });
    await expectNoViolations(section);
  });

  // AP-171, UI-44: a reader reaches Restore, is told why it is refused, and nothing is sent; a
  // refusal of the forge is shown in its own words.
  it("refuses Restore to a reader with the reason and shows the forge's refusal", async () => {
    const user = userEvent.setup();
    const reason = "Restore needs propose on App in project helsinki";
    const history = (allowed: boolean) => ({
      status: 200,
      body: {
        builds: [{ commit: "9a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b", current: false }],
        restore: allowed ? { allowed } : { allowed, reason },
        dataNote: "",
      },
    });
    const name = en.apps.build.history.restoreOne.replace("{commit}", "9a2b3c4");
    const { fetchMock, view } = renderPanel({ status: 200, body: built() }, undefined, { [`GET ${BUILDS}`]: history(false) });
    const refused = await within(await panel()).findByRole("button", { name });
    expect(refused).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText(reason)).toBeInTheDocument();
    await user.click(refused);
    expect(fetchMock.mock.calls.some(([input]) => (input as Request).method === "POST")).toBe(false);
    view.unmount();
    vi.unstubAllGlobals();

    const detail = "the default branch already holds the files of 9a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b";
    renderPanel({ status: 200, body: built() }, undefined, {
      [`GET ${BUILDS}`]: history(true),
      [`POST ${RESTORE}`]: { status: 409, body: { title: "Conflict", status: 409, detail } },
    });
    await user.click(await within(await panel()).findByRole("button", { name }));
    expect(await screen.findByText(detail)).toBeInTheDocument();
  });

  // PF-59, AP-100: nothing for an App the person may not read; one line for an App without a repository.
  it("draws nothing for an unreadable App and one line for an App not built on the forge", async () => {
    const { view } = renderPanel({ status: 404, body: { title: "Not Found", status: 404, detail: "app 'bikes' not found" } });
    await vi.waitFor(() => expect(view.container.innerHTML).toBe(""));
    view.unmount();
    vi.unstubAllGlobals();

    renderPanel({
      status: 200,
      body: { repositoryUrl: null, run: null, packageUrl: null, rebuild: { allowed: false, reason: "not on the forge" } },
    });
    expect(await screen.findByText(en.apps.build.notOnForge)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.apps.build.rebuild })).toBeNull();
  });
});
