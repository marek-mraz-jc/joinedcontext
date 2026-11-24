// T-7706: a name offered by two lists is shown once (project role over organization role), a job whose status
// cannot be read says so, and a knowledge source in more than three languages shows three with the rest counted.
import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { ResourceNamePicker } from "../src/components/pickers/ResourceNamePicker";
import { finishJob, resetJobs, startJob } from "../src/jobs";
import { json, renderPart } from "./page_contract";

const API = "joinedcontext.com/v1alpha1";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  localStorage.clear();
  resetJobs();
});

describe("ResourceNamePicker with two lists (T-7706)", () => {
  it("offers a name both lists hold once", async () => {
    const role = (name: string, namespace: string) => ({ apiVersion: API, kind: "Role", metadata: { name, namespace }, spec: { rules: [] } });
    renderPart(
      <ResourceNamePicker
        id="role"
        label="Role"
        from={[{ project: "helsinki", plural: "roles" }, { project: "org", plural: "roles" }]}
        value=""
        onChange={() => undefined}
      />,
      {
        answer: async (url) => {
          if (url.pathname === "/api/v1/projects/helsinki/roles") return json({ apiVersion: API, kind: "List", items: [role("editor", "helsinki")] });
          if (url.pathname === "/api/v1/projects/org/roles") return json({ apiVersion: API, kind: "List", items: [role("editor", "org"), role("viewer", "org")] });
          return undefined;
        },
      },
    );
    await userEvent.setup().click(await screen.findByRole("combobox", { name: "Role" }));
    const options = await screen.findAllByRole("option");
    expect(options.map((option) => option.textContent)).toEqual([expect.stringContaining("editor"), expect.stringContaining("viewer")]);
  });
});

describe("JobsMenu with a status that cannot be read (T-7706)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    localStorage.clear();
    resetJobs();
  });

  it("says the progress could not be read just now", async () => {
    vi.stubGlobal("fetch", () => Promise.resolve(json({ title: "Unavailable", status: 503, detail: "away" }, 503)));
    startJob({ kind: "appBuild", project: "helsinki", name: "bikes", after: 1 });
    const { JobsMenu } = await import("../src/components/JobsMenu");
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <JobsMenu />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await userEvent.setup().click(await screen.findByRole("button"));
    expect(await screen.findByText(en.jobs.unknown)).toBeInTheDocument();
    act(() => finishJob("none", "succeeded"));
  });
});

describe("the knowledge sources' languages (T-7706)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("shows the three largest and counts the rest", async () => {
    const source = {
      source: "bb-web", type: "website", state: "crawled", startUrls: ["https://www.banskabystrica.sk/"], ckanInstanceRef: null,
      schedule: null, visibility: "public", lastCrawl: "2026-10-06T03:00:00Z", pages: 10, pagesIncluded: 10, documents: 1, passages: 9, embedded: 9,
      pending: 0, failed: 0, languages: { fi: 420, en: 310, sv: 250, uk: 4, ru: 3, so: 2, ar: 1 }, job: { state: "done", attempts: 1, error: null },
    };
    vi.stubGlobal("fetch", (input: RequestInfo | URL) => {
      const url = new URL((input as Request).url);
      if (url.pathname.endsWith("/auth/me")) return Promise.resolve(json({ subject: "s", username: "jana.kovacova", name: "Jana", roles: [] }));
      if (url.pathname.endsWith("/permissions/me")) {
        return Promise.resolve(json({ project: "banskabystrica", bootstrap: false, grants: [{ rule: { kinds: ["KnowledgeSource"], verbs: ["read"] } }] }));
      }
      if (url.pathname.endsWith("/knowledge/sources")) return Promise.resolve(json({ items: [source] }));
      return Promise.resolve(json({ apiVersion: API, kind: "List", items: [] }));
    });
    window.history.pushState({}, "", "/projects/banskabystrica/knowledge");
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const table = await screen.findByRole("table", { name: "Knowledge sources" });
    const all = await within(table).findByText("fi 42%, en 31%, sv 25%, uk 0%, ru 0%, so 0%, ar 0%");
    expect(all.closest("span[title]")).toHaveAttribute("title", "fi 42%, en 31%, sv 25%, uk 0%, ru 0%, so 0%, ar 0%");
    expect(all.previousElementSibling?.textContent).toContain("+4 more");
  });
});
