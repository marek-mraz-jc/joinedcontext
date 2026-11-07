/**
 * T-5112: a dataset's quality score. The badge carries the number and its word, each part its
 * reason in the reader's language, a publisher reads what to fix, and a failed read says so.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import cs from "../src/locales/cs.json";
import de from "../src/locales/de.json";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";
import { QualityScore, SAID, toneOf } from "../src/components/quality/QualityScore";
import { EndpointQuality } from "../src/pages/endpoints/EndpointQuality";
import { expectAxeClean, jsonResponse, problem } from "./pageHarness";
import type { components } from "../src/api/schema";

// Every part the API names (`src/dataset_score.rs`): `satisfies` fails the typecheck on a missing or unknown one.
const PARTS = Object.keys({ freshness: 1, completeness: 1, validity: 1, links: 1 } satisfies
  Record<components["schemas"]["DatasetScorePart"]["part"], 1>) as components["schemas"]["DatasetScorePart"]["part"][];

const said = (code: string, text: string, args: Record<string, string> = {}) => ({ code, args, text });

const SCORE = {
  score: 48,
  measuredAt: "2026-10-22T08:04:00Z",
  parts: [
    {
      part: "freshness" as const,
      score: 0,
      measuredAt: "2026-10-22T08:04:00Z",
      reason: said("late", "1 of 1 feeds are late", { late: "1", total: "1", newest: "2026-10-21T02:50:00Z" }),
      fix: said("fixFailing", "English from the server", { pipeline: "weather", since: "2026-10-22T08:00:00Z" }),
    },
    { part: "completeness" as const, reason: said("noRequired", "the model marks no field required") },
    { part: "validity" as const, score: 95, reason: said("valid", "190 of 200", { valid: "190", checked: "200" }) },
    { part: "links" as const, score: 50, reason: said("linksUp", "1 of 2", { up: "1", total: "2" }) },
  ],
};

function withQueries(node: React.ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
}

describe("the quality score", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows_the_badge_the_parts_and_the_publisher_s_fix", async () => {
    const { container } = withQueries(<QualityScore score={SCORE} />);
    expect(screen.getByRole("region", { name: en.quality.score.title })).toBeInTheDocument();
    expect(screen.getByTestId("quality-badge")).toHaveTextContent("48 / 100 · poor");
    expect(screen.getByRole("meter", { name: en.quality.score.part.freshness })).toHaveAttribute("value", "0");
    expect(screen.getByRole("meter", { name: en.quality.score.part.completeness })).toHaveAttribute(
      "aria-valuetext",
      en.quality.score.unmeasured,
    );
    expect(screen.getByText("190 of 200 entities follow the model")).toBeInTheDocument();
    expect(
      screen.getByText(/^Pipeline weather fails since (?!2026-10-22T).+: open it and fix the step its newest run names$/),
    ).toBeInTheDocument();
    expect(screen.queryByText(/English from the server/)).toBeNull();
    await expectAxeClean(container);
  });

  it("reads_in_slovak_and_an_unknown_code_falls_back_to_the_server_s_words", async () => {
    await i18n.changeLanguage("sk");
    const score = { ...SCORE, parts: [{ part: "links" as const, score: 100, reason: said("newCode", "words from the server") }] };
    withQueries(<QualityScore score={score} />);
    expect(screen.getByRole("region", { name: sk.quality.score.title })).toBeInTheDocument();
    expect(screen.getByText("words from the server")).toBeInTheDocument();
  });

  // T-6565: a feed with no reading yet says so in words, in the reader's language, never "from never".
  it("a_late_feed_without_a_reading_says_none_has_arrived", async () => {
    const unread = said("lateUnread", "1 of 1 feeds are late; none has delivered an observation yet", { late: "1", total: "1" });
    const score = { ...SCORE, parts: [{ part: "freshness" as const, score: 0, reason: unread }] };
    const { unmount } = withQueries(<QualityScore score={score} />);
    expect(screen.getByText("1 of 1 feeds are late; none has delivered an observation yet")).toBeInTheDocument();
    expect(screen.queryByText(/never/)).toBeNull();
    unmount();
    await i18n.changeLanguage("sk");
    withQueries(<QualityScore score={score} />);
    expect(screen.getByText("1 z 1 zdrojov mešká; žiadny zatiaľ nedodal pozorovanie")).toBeInTheDocument();
  });

  it("grades_by_number", () => {
    expect([toneOf(undefined), toneOf(49), toneOf(50), toneOf(80)]).toEqual(["neutral", "danger", "warning", "success"]);
  });

  it("names_every_part_and_message_in_every_locale", () => {
    for (const locale of [en, sk, cs, de]) {
      for (const part of PARTS) expect(locale.quality.score.part[part]).toBeTruthy();
      for (const code of SAID) expect(locale.quality.score.say[code as keyof typeof en.quality.score.say]).toBeTruthy();
    }
  });

  it("an_endpoint_s_score_is_read_from_its_project_route", async () => {
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const url = input instanceof Request ? input.url : String(input);
      expect(new URL(url, window.location.origin).pathname).toBe("/api/v1/projects/bbsk/endpoints/air-api/quality");
      return Promise.resolve(jsonResponse(SCORE));
    });
    vi.stubGlobal("fetch", fetchMock);
    withQueries(<EndpointQuality project="bbsk" name="air-api" />);
    expect(await screen.findByTestId("quality-badge")).toHaveTextContent("48 / 100");
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("a_refused_read_says_so_in_one_line", async () => {
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(problem(404, "endpoint 'air-api' not found"))));
    withQueries(<EndpointQuality project="bbsk" name="air-api" />);
    expect(await screen.findByRole("status")).toHaveTextContent(en.quality.score.unavailable);
  });
});
