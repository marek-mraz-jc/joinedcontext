/**
 * T-4670 (API/01 §34, AG-99): Asistent → Konverzácie. The assistant's administrator reads the
 * stored conversations with their filter, opens one as its turns and steps, copies a turn as an
 * eval case and erases a conversation only with `delete`; a member reads none of it.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { narrowsAndOrders } from "./grid_checks";
import { pageFindings } from "./pageChecks";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { queryKeys } from "../src/api/client";
import { ConversationsPage } from "../src/pages/knowledge/ConversationsPage";
import { UsageSummary } from "../src/pages/knowledge/UsageSummary";
import {
  aligned,
  capped,
  costText,
  evalCase,
  filterQuery,
  msText,
  rangeQuery,
  shareText,
  tokensText,
  totals,
  usdText,
  visible,
  visualChanges,
  wordDiff,
} from "../src/pages/knowledge/conversations";
import type { ConversationStep } from "../src/pages/knowledge/conversations";

const PROJECT = "helsinki";
const ONE = "11111111-1111-4111-8111-111111111111";
const LIST = {
  items: [{
    id: ONE, deployment: "obcan", origin: null, startedAt: "2026-10-18T10:00:00Z", lastAt: "2026-10-18T10:01:00Z",
    firstQuestion: "Aké je PM10?", turns: 1, routes: ["data"], totalMs: 2400, errors: 0, timeouts: 0,
    costUsd: 0.0046, uncosted: 1, thumbsUp: 0, thumbsDown: 1,
    tokensIn: 58722, tokensOut: 235, cachedTokens: 400, firstTokenMs: 800, models: ["google/gemini-3.8-flash"],
  }, {
    id: "33333333-3333-4333-8333-333333333333", deployment: "obcan", origin: null, startedAt: "2026-10-18T09:00:00Z",
    lastAt: "2026-10-18T09:00:30Z", firstQuestion: "Kto je starosta?", turns: 1, routes: ["rag"], totalMs: 45000, errors: 0,
    timeouts: 1, costUsd: 0, uncosted: 1, thumbsUp: 0, thumbsDown: 0, tokensIn: 5560, tokensOut: 185, cachedTokens: null,
    firstTokenMs: null, models: [], lastError: "The assistant took too long.",
  }],
  nextCursor: null,
};
const DETAIL = {
  id: ONE, deployment: "obcan", origin: null,
  turns: [{
    turn: 1, at: "2026-10-18T10:01:00Z", question: "Aké je PM10?", answer: "PM10 je dnes 31 µg/m³.", route: "data",
    firstTokenMs: 800, totalMs: 2400, error: null, timedOut: false, costUsd: 0.0046, uncosted: 1, verdict: "down",
    steps: [
      { kind: "model", name: "data", endpoint: null, input: null, output: null, outputBytes: null, status: "ok", startMs: 0,
        durationMs: 900, error: null, stage: "data", model: "google/gemini-3.8-flash", provider: null, tokensIn: 1000,
        tokensOut: 100, reasoningTokens: null, costUsd: 0.0046 },
      { kind: "tool", name: "query_entities", endpoint: "hronov-air", input: "{\"type\":\"AirQualityObserved\"}",
        output: "[{\"pm10\":31}]", outputBytes: 13, status: "failed", startMs: 900, durationMs: 120, error: "timeout",
        stage: null, model: null, provider: null, tokensIn: null, tokensOut: null, reasoningTokens: null, costUsd: null },
      { kind: "model", name: "answer", endpoint: null, input: null, output: null, outputBytes: null, status: "ok", startMs: 1100,
        durationMs: 700, error: null, stage: "answer", model: "unpriced/model", provider: null, tokensIn: 500,
        tokensOut: 80, reasoningTokens: null, costUsd: null },
    ],
  }],
};

// T-4681: a second run of the same question, its stored text hostile.
const TWO = "22222222-2222-4222-8222-222222222222";
const HOSTILE = [
  "<script>window.pwned = 1</script>",
  "<img src=x onerror=\"window.pwned = 1\">",
  "[x](javascript:window.pwned=1)",
  "<svg onload=\"window.pwned = 1\"></svg>",
  "evil\u202Etxt.exe",
].join(" ");
const step = (kind: ConversationStep["kind"], extra: Partial<ConversationStep> = {}): ConversationStep => ({
  kind, name: kind, endpoint: null, input: null, output: null, outputBytes: null, status: "ok", startMs: 0, durationMs: 100,
  error: null, stage: null, model: null, provider: null, tokensIn: null, tokensOut: null, reasoningTokens: null, costUsd: null,
  ...extra,
});
const OTHER = {
  id: TWO, deployment: "obcan", origin: null,
  turns: [{
    turn: 1, at: "2026-10-19T10:01:00Z", question: "Aké je PM10?", answer: `PM10 je dnes 40 µg/m³. ${HOSTILE}`, route: "data",
    firstTokenMs: 700, totalMs: 3100, error: null, timedOut: false, costUsd: 0.006, uncosted: 0, verdict: null,
    steps: [
      step("model", { model: "google/gemini-3.8-flash", tokensIn: 1000, tokensOut: 90, cachedTokens: 800, costUsd: 0.006,
        reasoning: HOSTILE }),
      step("tool", { name: "query_entities", input: HOSTILE, output: "x".repeat(70 * 1024), rows: 20 }),
      step("visual", { name: "chart", input: "{\"mark\":\"bar\"}", rows: 7 }),
    ],
  }],
};
const spreadOf = (p50: number | null, p90: number | null, max: number | null) => ({ p50, p90, max });
const USAGE = {
  questions: 21, timeouts: 7, errors: 1, uncostedQuestions: 1,
  costUsd: { total: 0.2082, mean: 0.0104, p50: 0.0099, p90: 0.0151 },
  tokens: { in: 58722, out: 235, cached: 1200, cachedShare: 0.2 },
  answerCache: { hits: 3, hitShare: 0.1429, savedMs: 26400, savedUsd: 0.0312 },
  firstTokenMs: spreadOf(8100, 20000, 30000), totalMs: spreadOf(28900, 45000, 45200),
  phases: { route: spreadOf(1200, 2100, 3000), work: spreadOf(9000, 15000, 20000), answer: spreadOf(6000, 9000, 12000) },
  costByStage: [
    { stage: "answer", calls: 20, costUsd: 0.15, uncostedCalls: 1 },
    { stage: "router", calls: 21, costUsd: null, uncostedCalls: 21 },
  ],
  days: [{ day: "2026-10-19", questions: 21, costUsd: 0.2082, uncostedQuestions: 1, p50TotalMs: 28900, timeouts: 7, errors: 1 }],
};
const CSV = "at,question\r\n2026-10-19T07:00:00Z,Kedy?\r\n";

const SAME = {
  items: [
    { conversation: TWO, turn: 1, at: "2026-10-19T10:01:00Z", deployment: "obcan", totalMs: 3100, costUsd: 0.006, steps: 3 },
    { conversation: ONE, turn: 1, at: "2026-10-18T10:01:00Z", deployment: "obcan", totalMs: 2400, costUsd: 0.0046, steps: 3 },
  ],
};

/** The project's assistants, as the picker reads them; `[]` is a project without one. */
let deployments: string[] = ["obcan"];

function renderAs(verbs: string[], link: { open?: string; against?: string } = {}) {
  const calls: { method: string; url: URL }[] = [];
  const permissions = {
    bootstrap: false, project: PROJECT,
    grants: [{ role: "r", binding: "b", scope: PROJECT, space: null, rule: { kinds: ["AssistantDeployment"], verbs } }],
  };
  vi.stubGlobal("fetch", (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(String(input), init);
    const url = new URL(request.url, "http://localhost");
    calls.push({ method: request.method, url });
    if (url.pathname.endsWith("/conversations/export")) {
      return Promise.resolve(new Response(CSV, { status: 200, headers: { "Content-Type": "text/csv; charset=utf-8" } }));
    }
    const body = request.method === "DELETE" ? null
      : url.pathname.endsWith("/permissions/me") ? permissions
      : url.pathname.endsWith("/conversations/usage") ? USAGE
      : url.pathname.endsWith("/assistantdeployments")
        ? { items: deployments.map((name) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "AssistantDeployment", metadata: { name, namespace: PROJECT, title: { en: "Citizen desk" } }, spec: {} })) }
      : url.pathname.endsWith("/same") ? SAME
      : url.pathname.endsWith(TWO) ? OTHER
      : url.pathname.endsWith(ONE) ? DETAIL : LIST;
    return Promise.resolve(
      new Response(body === null ? null : JSON.stringify(body), {
        status: body === null ? 204 : 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(queryKeys.permissions(PROJECT), permissions);
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ConversationsPage project={PROJECT} {...link} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return calls;
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("the stored conversations' helpers", () => {
  it("send only the filter values that were set, and q from two characters", () => {
    expect(filterQuery({})).toEqual({});
    expect(filterQuery({ q: " a ", slow: false, sort: "lastAt" })).toEqual({});
    expect(filterQuery({ q: " pm10 ", status: "timeout", slow: true, sort: "costUsd", cursor: "50" }))
      .toEqual({ q: "pm10", status: "timeout", slow: true, sort: "costUsd", cursor: "50" });
    // A day is its whole UTC day: `to` is the start of the next one, exclusive.
    expect(filterQuery({ deployment: " obcan ", from: "2026-10-01", to: "2026-10-31", expensive: true }))
      .toEqual({ deployment: "obcan", from: "2026-10-01T00:00:00Z", to: "2026-11-01T00:00:00Z", minCostUsd: 0.02 });
    expect(filterQuery({ from: "yesterday", to: "" })).toEqual({});
  });

  it("copy a turn as an eval case of the open-data set", () => {
    expect(JSON.parse(evalCase({ question: "Aké je PM10?", route: "data" }, "sk")))
      .toEqual({ id: "od-00", lang: "sk", kind: "value", q: "Aké je PM10?", must: [] });
    expect(JSON.parse(evalCase({ question: "Datasety?", route: "rag" }, "de")).lang).toBe("sk");
  });

  it("write times and prices as a person reads them", () => {
    expect([msText(850), msText(2400), msText(null)]).toEqual(["850 ms", "2.4 s", "–"]);
    expect([usdText(0.0046), usdText(1.5), usdText(null)]).toEqual(["$0.0046", "$1.50", "–"]);
  });
});

describe("the usage summary's helpers (T-4728)", () => {
  it("say the price is unknown for an unpriced turn, never $0, and format tokens and shares", () => {
    expect(costText(0, 1, "cena neznáma")).toBe("cena neznáma");
    expect(costText(null, 2, "cena neznáma")).toBe("cena neznáma");
    expect(costText(0.0046, 1, "cena neznáma")).toBe("$0.0046");
    expect(costText(0, 0, "cena neznáma")).toBe("$0.0000");
    expect(tokensText(58722, "en")).toBe("58,722");
    expect(tokensText(null, "en")).toBe("–");
    expect(shareText(0.2, "en")).toBe("20%");
    expect(shareText(null, "en")).toBe("–");
  });

  it("send the summary and the export the list's deployment and days only", () => {
    expect(rangeQuery({ deployment: " obcan ", from: "2026-10-18", to: "2026-10-18", status: "timeout", q: "PM10" })).toEqual({
      deployment: "obcan", from: "2026-10-18T00:00:00Z", to: "2026-10-19T00:00:00Z",
    });
    expect(rangeQuery({})).toEqual({});
  });
});

describe("the compare view's helpers (T-4681)", () => {
  it("align two timelines by kind, unmatched steps facing an empty cell", () => {
    const left = [step("route"), step("tool"), step("model"), step("answer")];
    const right = [step("route"), step("model"), step("visual"), step("answer")];
    expect(aligned(left, right).map(([a, b]) => [a?.kind ?? null, b?.kind ?? null])).toEqual([
      ["route", "route"],
      ["tool", null],
      ["model", "model"],
      [null, "visual"],
      ["answer", "answer"],
    ]);
    expect(aligned([], [step("tool")])).toEqual([[null, expect.objectContaining({ kind: "tool" })]]);
    expect(aligned([], [])).toEqual([]);
  });

  it("diff two answers word by word", () => {
    expect(wordDiff("PM10 je 31 dnes", "PM10 je 40 dnes")).toEqual([
      { word: "PM10", change: "same" },
      { word: "je", change: "same" },
      { word: "31", change: "removed" },
      { word: "40", change: "added" },
      { word: "dnes", change: "same" },
    ]);
    expect(wordDiff("", "")).toEqual([]);
    // Past the word cap both answers show as wholly changed instead of an O(n·m) table.
    const long = Array.from({ length: 2500 }, (_, i) => `w${i}`).join(" ");
    const diff = wordDiff(long, long);
    expect(diff.filter((w) => w.change === "removed")).toHaveLength(2500);
    expect(diff.filter((w) => w.change === "added")).toHaveLength(2500);
  });

  it("total a turn's steps, calls, time, cost and cached share", () => {
    const turn = OTHER.turns[0];
    expect(totals(turn)).toEqual({ steps: 3, tools: 1, models: 1, totalMs: 3100, costUsd: 0.006, cachedShare: 0.8 });
    expect(totals({ steps: [], totalMs: 0, costUsd: 0 }).cachedShare).toBeNull();
  });

  it("name the visuals' spec changes: another chart type, a missing unit, a different point count", () => {
    const a = [step("visual", { name: "chart", input: JSON.stringify({ mark: { type: "line" }, units: { pm10: "µg/m³" } }), rows: 12 })];
    const b = [step("visual", { name: "chart", input: JSON.stringify({ mark: "bar" }), rows: 7 }), step("visual", { name: "map" })];
    expect(visualChanges(a, b)).toEqual([
      "mark: line → bar",
      "units: {\"pm10\":\"µg/m³\"} → –",
      "rows: 12 → 7",
      "– → map",
    ]);
    // A spec cut at 64 KB no longer parses: the kind and rows still compare.
    expect(visualChanges([step("visual", { name: "table", input: "{\"cut", rows: 3 })], [step("visual", { name: "table", rows: 3 })]))
      .toEqual([]);
  });

  it("show bidi and zero-width characters as markers and cap long text", () => {
    expect(visible("evil\u202Etxt\u200B.exe\uFEFF")).toBe("evil⟦U+202E⟧txt⟦U+200B⟧.exe⟦U+FEFF⟧");
    expect(visible("Banská Bystrica")).toBe("Banská Bystrica");
    expect(capped("abc", 2)).toEqual({ text: "ab", more: true });
    expect(capped("ab", 2)).toEqual({ text: "ab", more: false });
    const huge = capped("x".repeat(10 * 1024 * 1024));
    expect(huge.more).toBe(true);
    expect(huge.text).toHaveLength(64 * 1024);
  });
});

describe("Asistent → Konverzácie", () => {
  it("a member without approve reads nothing and the assistant is never asked", async () => {
    const calls = renderAs(["read"]);
    expect(await screen.findByText(i18n.t("conversations.notAllowed"))).toBeInTheDocument();
    // T-5328: the refused page still says what it is for under its title.
    expect(screen.getByRole("heading", { level: 1, name: "Conversations" }).nextElementSibling).toHaveTextContent(i18n.t("conversations.intro"));
    expect(calls.filter((c) => c.url.pathname.includes("/conversations"))).toEqual([]);
  });

  it("asks the recorded list once per typed search, not once per key (F-R2UI3-009)", async () => {
    const user = userEvent.setup();
    const calls = renderAs(["read", "approve"]);
    await screen.findByRole("table");
    await user.type(screen.getByLabelText(i18n.t("conversations.filter.q")), "parking");
    await waitFor(() => expect(calls.some((c) => c.url.searchParams.get("q") === "parking")).toBe(true));
    expect(calls.filter((c) => c.url.searchParams.has("q")).map((c) => c.url.searchParams.get("q"))).toEqual(["parking"]);
  });

  it("the administrator filters, opens a conversation with its steps and copies a turn; erase needs delete", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const calls = renderAs(["read", "approve"]);
    const table = await screen.findByRole("table");
    expect(within(table).getByText("Aké je PM10?")).toBeInTheDocument();
    expect(within(table).getByText("$0.0046")).toBeInTheDocument();

    await user.selectOptions(screen.getByRole("combobox", { name: i18n.t("conversations.filter.status") }), "timeout");
    await waitFor(() => {
      expect(calls.some((c) => c.url.searchParams.get("status") === "timeout")).toBe(true);
    });

    // Each row holds its record's link, which the walker's row-link check asks for (T-6322).
    expect(within(screen.getByRole("table")).getByRole("link", { name: "Aké je PM10?" })).toHaveAttribute("href", `?open=${ONE}`);
    expect(pageFindings({ namespaces: [], layout: false }).filter((finding) => finding.startsWith("row-link"))).toEqual([]);
    await user.click(within(screen.getByRole("table")).getByRole("link", { name: "Aké je PM10?" }));
    expect(await screen.findByText("PM10 je dnes 31 µg/m³.")).toBeInTheDocument();
    // The list row names the model too; the transcript's step is the one outside the table.
    const outside = screen.getAllByText(/google\/gemini-3.8-flash/).filter((e) => !screen.getByRole("table").contains(e));
    expect(outside).toHaveLength(1);
    expect(screen.getByText(/hronov-air/)).toBeInTheDocument();
    // A call with no price says so, never $0 (T-4178).
    expect(screen.getByText(/unpriced\/model/).textContent).toContain(i18n.t("conversations.unknownPrice"));
    expect(screen.queryByText(/\$0\.0000/)).toBeNull();
    expect(screen.queryByRole("button", { name: i18n.t("conversations.erase") })).toBeNull();

    await user.click(screen.getByRole("button", { name: i18n.t("conversations.copyAsTest") }));
    expect(writeText).toHaveBeenCalledOnce();
    expect(JSON.parse((writeText.mock.calls[0] as unknown as [string])[0]).q).toBe("Aké je PM10?");
  });

  // T-6206: the page's rows are on the shared grid: search, sort by tokens, filter by status.
  it("searches, sorts by tokens and filters by status on the shared grid", async () => {
    renderAs(["read", "approve"]);
    const table = await screen.findByRole("table");
    await within(table).findByText("Kto je starosta?");
    await narrowsAndOrders(table, {
      search: { label: `Search ${i18n.t("conversations.title")}`, text: "starosta", kept: ["Kto je starosta?"] },
      sort: { column: i18n.t("conversations.column.tokens"), ascending: ["Kto je starosta?", "Aké je PM10?"] },
      filter: { value: new RegExp(`^${i18n.t("conversations.status.timeout")}`), kept: ["Kto je starosta?"] },
    });
  });

  it("shows the usage summary, the rows' tokens and times, sorts by tokens and downloads the CSV (T-4728)", async () => {
    const user = userEvent.setup();
    const saved: Blob[] = [];
    const createObjectURL = vi.fn((blob: Blob) => {
      saved.push(blob);
      return "blob:csv";
    });
    Object.defineProperty(URL, "createObjectURL", { value: createObjectURL, configurable: true });
    Object.defineProperty(URL, "revokeObjectURL", { value: vi.fn(), configurable: true });
    const calls = renderAs(["read", "approve"]);
    const summary = await screen.findByLabelText(i18n.t("conversations.usage.title"));
    expect(await within(summary).findByText("21", { selector: "dd" })).toBeInTheDocument();
    expect(within(summary).getByText("$0.21")).toBeInTheDocument();
    expect(within(summary).getByText(i18n.t("conversations.usage.uncosted", { count: 1 }))).toBeInTheDocument();
    // T-4731: the answer cache's hits and what they saved
    expect(within(summary).getByText(i18n.t("conversations.usage.answerCache"))).toBeInTheDocument();
    expect(
      within(summary).getByText(i18n.t("conversations.usage.answerCacheNote", { hits: 3, ms: "26.4 s", usd: "$0.03" })),
    ).toBeInTheDocument();
    expect(within(summary).getByText("28.9 s")).toBeInTheDocument();
    expect(within(summary).getByText(/2026-10-19/)).toBeInTheDocument();
    // A stage nothing priced says so, never $0.
    expect(within(summary).getByText(/router:/).textContent).toContain(i18n.t("conversations.unknownPrice"));

    const table = await screen.findByRole("table");
    expect(within(table).getByText("58,722 / 235")).toBeInTheDocument();
    expect(within(table).getByText("google/gemini-3.8-flash")).toBeInTheDocument();
    expect(within(table).getByText("800 ms")).toBeInTheDocument();
    const unpriced = within(table).getByText("Kto je starosta?").closest("tr");
    expect(unpriced?.textContent).toContain(i18n.t("conversations.unknownPrice"));
    expect(unpriced?.textContent).not.toContain("$0.0000");
    // T-4994: a conversation that ended badly says what failed in its row.
    expect(within(unpriced as HTMLElement).getByText("The assistant took too long.")).toBeInTheDocument();

    await user.selectOptions(screen.getByLabelText(i18n.t("conversations.filter.sort")), "tokens");
    await waitFor(() => {
      expect(calls.some((c) => c.url.searchParams.get("sort") === "tokens")).toBe(true);
    });
    await user.type(screen.getByLabelText(i18n.t("conversations.filter.deployment")), "citi");
    await user.click(await screen.findByRole("option", { name: /Citizen desk/ }));
    await waitFor(() => {
      expect(calls.some((c) => c.url.pathname.endsWith("/usage") && c.url.searchParams.get("deployment") === "obcan")).toBe(true);
    });
    expect(calls.filter((c) => c.url.pathname.endsWith("/usage")).every((c) => !c.url.searchParams.has("sort"))).toBe(true);

    await user.click(within(summary).getByRole("button", { name: i18n.t("conversations.usage.export") }));
    await waitFor(() => expect(createObjectURL).toHaveBeenCalledOnce());
    expect(await saved[0].text()).toBe(CSV);
    expect(calls.some((c) => c.url.pathname.endsWith("/conversations/export"))).toBe(true);
  });

  it("filters by an assistant picked from the project's own, and says how to add one when it has none (T-4948)", async () => {
    const user = userEvent.setup();
    deployments = ["obcan"];
    const calls = renderAs(["read", "approve"]);
    const picker = await screen.findByRole("combobox", { name: i18n.t("conversations.filter.deployment") });
    await user.click(picker);
    await user.click(await screen.findByRole("option", { name: /Citizen desk/ }));
    await waitFor(() => {
      expect(calls.some((c) => c.url.pathname.endsWith("/conversations") && c.url.searchParams.get("deployment") === "obcan")).toBe(true);
    });
    expect((picker as HTMLInputElement).value).toBe("Citizen desk");
  });

  it("says how to add an assistant when the project has none (T-4948)", async () => {
    const user = userEvent.setup();
    deployments = [];
    renderAs(["read", "approve"]);
    await user.click(await screen.findByRole("combobox", { name: i18n.t("conversations.filter.deployment") }));
    expect(await screen.findByText(/Add one under Assistant → Deployments/)).toBeInTheDocument();
    deployments = ["obcan"];
  });

  it("one who may delete erases a conversation after confirming", async () => {
    const user = userEvent.setup();
    const calls = renderAs(["read", "approve", "delete"]);
    await user.click(await screen.findByRole("link", { name: "Aké je PM10?" }));
    await user.click(await screen.findByRole("button", { name: i18n.t("conversations.erase") }));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: i18n.t("conversations.erase") }));
    await waitFor(() => {
      expect(calls.some((c) => c.method === "DELETE" && c.url.pathname.endsWith(`/conversations/${ONE}`))).toBe(true);
    });
  });

  it("lists the same question's turns and compares two side by side, every stored text inert", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const calls = renderAs(["read", "approve"]);
    await user.click(await screen.findByRole("link", { name: "Aké je PM10?" }));
    await user.click(await screen.findByRole("button", { name: i18n.t("conversations.same") }));
    const same = await screen.findByRole("list", { name: i18n.t("conversations.same") });
    // The turn itself is not offered against itself.
    expect(within(same).getAllByRole("button", { name: i18n.t("conversations.compare") })).toHaveLength(1);
    expect(calls.some((c) => c.url.pathname.endsWith(`/conversations/${ONE}/turns/1/same`))).toBe(true);

    await user.click(within(same).getByRole("button", { name: i18n.t("conversations.compare") }));
    const view = await screen.findByRole("region", { name: i18n.t("conversations.compareTitle") });
    expect(await within(view).findByTestId("compare-right-totals")).toHaveTextContent("3 steps · 1 tools · 1 model calls · 3.1 s · $0.0060 · cached 80 %");
    expect(within(view).getByTestId("compare-left-totals")).toHaveTextContent("3 steps · 1 tools · 2 model calls · 2.4 s · $0.0046 · cached 0 %");
    expect(within(view).getByRole("list", { name: i18n.t("conversations.changes") })).toHaveTextContent("– → chart");
    const diff = within(view).getByTestId("answer-diff");
    expect(diff.querySelector("del")).toHaveTextContent("31");
    expect(diff.querySelector("ins")).toHaveTextContent("40");

    // Hostile stored text stays text: no element, no handler, no link, the bidi override marked.
    for (const node of [view, ...Array.from(view.querySelectorAll("details"))]) {
      if (node instanceof HTMLDetailsElement) node.open = true;
    }
    expect(view.querySelector("script, img, svg[onload], a[href^='javascript']")).toBeNull();
    expect(view.textContent).toContain("<script>window.pwned = 1</script>");
    expect(view.textContent).toContain("evil⟦U+202E⟧txt.exe");
    expect(view.textContent).not.toContain("\u202E");
    expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
    // A 70 KB result shows its first 64 KB until "Zobraziť viac".
    const more = within(view).getByRole("button", { name: i18n.t("conversations.showMore", { size: 70 }) });
    await user.click(more);
    expect(within(view).queryByRole("button", { name: i18n.t("conversations.showMore", { size: 70 }) })).toBeNull();

    await user.click(within(view).getByRole("button", { name: i18n.t("conversations.copyPair") }));
    const pair = JSON.parse((writeText.mock.calls[0] as unknown as [string])[0]) as { question: string }[];
    expect(pair.map((turn) => turn.question)).toEqual(["Aké je PM10?", "Aké je PM10?"]);
  });

  it("an eval run's link opens its conversation against the previous run's last turn", async () => {
    renderAs(["read", "approve"], { open: ONE, against: TWO });
    const view = await screen.findByRole("region", { name: i18n.t("conversations.compareTitle") });
    expect(await within(view).findByTestId("compare-right-totals")).toHaveTextContent("cached 80 %");
    await userEvent.setup().click(within(view).getByRole("button", { name: i18n.t("conversations.compareClose") }));
    expect(screen.queryByRole("region", { name: i18n.t("conversations.compareTitle") })).toBeNull();
  });
});

describe("UsageSummary (T-4728)", () => {
  it("offers to forget one assistant's kept answers and says how many went (T-4731)", async () => {
    const user = userEvent.setup();
    const deletes: string[] = [];
    vi.stubGlobal("fetch", (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init);
      const url = new URL(request.url, "http://localhost");
      if (request.method === "DELETE") deletes.push(url.pathname);
      const body = request.method === "DELETE" ? { forgotten: 4 } : USAGE;
      return Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } }));
    });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { rerender } = render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <UsageSummary project={PROJECT} filter={{}} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const summary = await screen.findByLabelText(i18n.t("conversations.usage.title"));
    // Without one assistant chosen there is nothing to forget.
    expect(within(summary).queryByRole("button", { name: i18n.t("conversations.usage.forget") })).toBeNull();
    rerender(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <UsageSummary project={PROJECT} filter={{ deployment: "obcan" }} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await user.click(await within(summary).findByRole("button", { name: i18n.t("conversations.usage.forget") }));
    expect(await within(summary).findByRole("status")).toHaveTextContent(i18n.t("conversations.usage.forgot", { count: 4 }));
    expect(deletes).toEqual([`/api/v1/projects/${PROJECT}/knowledge/deployments/obcan/answers`]);
  });

  it("reads an all-unpriced range as an unknown price and shows the export's refusal in words", async () => {
    const user = userEvent.setup();
    const unpriced = {
      ...USAGE,
      uncostedQuestions: 3,
      costUsd: { total: null, mean: null, p50: null, p90: null },
      costByStage: [],
      days: [],
    };
    vi.stubGlobal("fetch", (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(String(input), init);
      const refused = new URL(request.url, "http://localhost").pathname.endsWith("/export");
      const body = refused
        ? { type: "about:blank", title: "Bad Request", status: 400, detail: "more than 20000 turns: narrow the range" }
        : unpriced;
      return Promise.resolve(new Response(JSON.stringify(body), {
        status: refused ? 400 : 200,
        headers: { "Content-Type": refused ? "application/problem+json" : "application/json" },
      }));
    });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <UsageSummary project={PROJECT} filter={{}} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const summary = await screen.findByLabelText(i18n.t("conversations.usage.title"));
    // T-4994: one line says the price is missing; no cost tile repeats "unknown".
    expect(await within(summary).findByText(i18n.t("conversations.usage.notRecorded.cost"))).toBeInTheDocument();
    expect(within(summary).queryByText(i18n.t("conversations.unknownPrice"))).toBeNull();
    expect(within(summary).queryByText(i18n.t("conversations.usage.cost"))).toBeNull();
    expect(summary.textContent).not.toContain("$0.00");
    await user.click(within(summary).getByRole("button", { name: i18n.t("conversations.usage.export") }));
    expect(await within(summary).findByText(/narrow the range/)).toBeInTheDocument();
  });

  function summaryOf(body: unknown) {
    vi.stubGlobal("fetch", () =>
      Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } })),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <UsageSummary project={PROJECT} filter={{}} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    return screen.findByLabelText(i18n.t("conversations.usage.title"));
  }

  it("says an empty range has no questions instead of a wall of dashes (T-4994)", async () => {
    const none = spreadOf(null, null, null);
    const summary = await summaryOf({
      ...USAGE, questions: 0, timeouts: 0, errors: 0, uncostedQuestions: 0,
      costUsd: { total: null, mean: null, p50: null, p90: null },
      tokens: { in: null, out: null, cached: null, cachedShare: null },
      answerCache: { hits: 0, hitShare: null, savedMs: null, savedUsd: null },
      firstTokenMs: none, totalMs: none, phases: { route: none, work: none, answer: none }, costByStage: [], days: [],
    });
    expect(await within(summary).findByText(i18n.t("conversations.usage.none"))).toBeInTheDocument();
    expect(within(summary).queryByText(i18n.t("conversations.usage.questions"))).toBeNull();
    expect(summary.textContent).not.toContain("–");
  });

  it("says once that cost and tokens were not recorded and leaves out the speeds nobody measured (T-4994)", async () => {
    const none = spreadOf(null, null, null);
    const summary = await summaryOf({
      ...USAGE, questions: 3, timeouts: 0, errors: 3, uncostedQuestions: 3,
      costUsd: { total: null, mean: null, p50: null, p90: null },
      tokens: { in: 0, out: 0, cached: null, cachedShare: null },
      answerCache: { hits: 0, hitShare: 0, savedMs: null, savedUsd: null },
      firstTokenMs: none, totalMs: spreadOf(3000, 3000, 3000),
      phases: { route: none, work: spreadOf(3000, 3000, 3000), answer: none },
      costByStage: [{ stage: "agent", calls: 3, costUsd: null, uncostedCalls: 3 }],
      days: [],
    });
    expect(await within(summary).findByText(i18n.t("conversations.usage.notRecorded.both"))).toBeInTheDocument();
    for (const gone of ["cost", "perQuestion", "tokens", "firstToken", "answerCache", "phase.route", "phase.answer", "byStage"]) {
      expect(within(summary).queryByText(i18n.t(`conversations.usage.${gone}`))).toBeNull();
    }
    expect(within(summary).getByText(i18n.t("conversations.usage.phase.work"))).toBeInTheDocument();
    expect(within(summary).queryByText(i18n.t("conversations.unknownPrice"))).toBeNull();
    expect(summary.textContent).not.toContain("–");
  });

  it("says a body of another shape failed instead of breaking the page", async () => {
    vi.stubGlobal("fetch", () =>
      Promise.resolve(new Response(JSON.stringify({ items: [] }), { status: 200, headers: { "Content-Type": "application/json" } })),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <UsageSummary project={PROJECT} filter={{}} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const summary = await screen.findByLabelText(i18n.t("conversations.usage.title"));
    expect(await within(summary).findByRole("alert")).toBeInTheDocument();
    expect(within(summary).getByRole("button", { name: i18n.t("conversations.usage.export") })).toBeEnabled();
  });
});
