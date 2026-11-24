// T-7706: AssistantPage gaps: the state filter (running, ended), Only mine, and the retry of a failed run list.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";

const PROJECT = "banskabystrica";
const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-approver"],
  groups: [PROJECT],
};

const run = (id: string, prompt: string, status: string) => ({
  id,
  project: PROJECT,
  appName: "",
  endpointName: "",
  endpointSlug: "",
  profile: "app-builder",
  appClass: "static",
  visibility: "private",
  kind: "conversation",
  unattended: false,
  continues: null,
  prompt,
  status,
  steps: 1,
  tokensUsed: 10,
  createdBy: "jana.kovacova",
  createdAt: "2026-09-14T09:00:00Z",
});
const ENDED = run("run-ended", "An ended conversation", "cancelled");
const LIVE = run("run-live", "A running conversation", "interviewing");

class StubEventSource {
  addEventListener(): void {}
  removeEventListener(): void {}
  close(): void {}
}

let requests: URL[] = [];

function renderAssistant(options: { failFirst?: boolean } = {}) {
  requests = [];
  let failures = options.failFirst ? 1 : 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(input, init);
      const url = new URL(request.url, "http://localhost");
      const path = url.pathname;
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), {
          status,
          headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" },
        });
      if (path.endsWith("/auth/me")) return json(IDENTITY);
      if (path.endsWith("/projects")) return json([PROJECT]);
      if (path.endsWith("/agent-runs")) {
        requests.push(url);
        if (failures > 0) {
          failures -= 1;
          return json({ title: "Unavailable", status: 503, detail: "the run store is restarting" }, 503);
        }
        return json({ items: [ENDED, LIVE] });
      }
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
  vi.stubGlobal("EventSource", StubEventSource as unknown as typeof EventSource);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the run list's filters", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.sessionStorage.clear();
    window.history.pushState({}, "", `/projects/${PROJECT}/assistant`);
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("shows only the running conversations, then only the ended ones", async () => {
    const user = userEvent.setup();
    renderAssistant();
    const table = await screen.findByRole("table");
    expect(await within(table).findByText("An ended conversation")).toBeInTheDocument();
    expect(within(table).getByText("A running conversation")).toBeInTheDocument();

    await user.selectOptions(screen.getByLabelText(en.assistantPage.filters.status), "live");
    expect(within(screen.getByRole("table")).queryByText("An ended conversation")).toBeNull();
    expect(within(screen.getByRole("table")).getByText("A running conversation")).toBeInTheDocument();

    await user.selectOptions(screen.getByLabelText(en.assistantPage.filters.status), "ended");
    expect(within(screen.getByRole("table")).getByText("An ended conversation")).toBeInTheDocument();
    expect(within(screen.getByRole("table")).queryByText("A running conversation")).toBeNull();
  });

  it("asks the server for the person's own runs when Only mine is ticked", async () => {
    const user = userEvent.setup();
    renderAssistant();
    await screen.findByRole("table");
    await user.click(screen.getByRole("checkbox", { name: en.assistantPage.filters.mine }));
    await waitFor(() => expect(requests.some((url) => url.searchParams.has("mine"))).toBe(true));
  });

  it("says why the runs could not be listed and reads them again on retry", async () => {
    const user = userEvent.setup();
    renderAssistant({ failFirst: true });
    expect(await screen.findByText(/the run store is restarting/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.form.listRetry }));
    expect(await screen.findByText("An ended conversation")).toBeInTheDocument();
    expect(requests.length).toBeGreaterThanOrEqual(2);
  });
});
