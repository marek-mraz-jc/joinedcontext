// T-7706: UsageSummary: a priced range without provider tokens says so once, and a refused "forget answers" shows the reason.
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { UsageSummary } from "../src/pages/knowledge/UsageSummary";

const spread = (p50: number | null, p90: number | null, max: number | null) => ({ p50, p90, max });
const USAGE = {
  questions: 4, timeouts: 0, errors: 0, uncostedQuestions: 0,
  costUsd: { total: 0.05, mean: 0.0125, p50: 0.01, p90: 0.02 },
  tokens: { in: null, out: null, cached: null, cachedShare: null },
  answerCache: { hits: 0, hitShare: 0, savedMs: 0, savedUsd: 0 },
  firstTokenMs: spread(null, null, null), totalMs: spread(1000, 2000, 3000),
  phases: { route: spread(null, null, null), work: spread(null, null, null), answer: spread(null, null, null) },
  costByStage: [],
  days: [],
};

afterEach(() => vi.unstubAllGlobals());

describe("UsageSummary edges (T-7706)", () => {
  it("says the tokens were not recorded when the range is priced but the provider reported none, and shows a refused forget", async () => {
    vi.stubGlobal("fetch", (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init);
      const forget = request.method === "DELETE";
      return Promise.resolve(new Response(
        JSON.stringify(forget ? { title: "Forbidden", status: 403, detail: "no role grants delete on AssistantDeployment" } : USAGE),
        { status: forget ? 403 : 200, headers: { "Content-Type": forget ? "application/problem+json" : "application/json" } },
      ));
    });
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <UsageSummary project="helsinki" filter={{ deployment: "obcan" }} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const summary = await screen.findByLabelText(i18n.t("conversations.usage.title"));
    expect(await within(summary).findByText(i18n.t("conversations.usage.notRecorded.tokens"))).toBeInTheDocument();
    await userEvent.setup().click(await within(summary).findByRole("button", { name: i18n.t("conversations.usage.forget") }));
    expect(await within(summary).findByText(/no role grants delete on AssistantDeployment/)).toBeInTheDocument();
    expect(within(summary).queryByRole("status")).toBeNull();
  });
});
