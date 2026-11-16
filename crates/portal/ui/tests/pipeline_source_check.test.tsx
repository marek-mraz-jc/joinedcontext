/**
 * T-7085: the workbench's whole-source check (G-21). A person runs the saved pipeline over every
 * page of its source, reads how many records a live run would accept and reject, by rule, with
 * where each fix belongs, and the first rejected records; a refusal shows the Portal's words, and
 * a pipeline not yet saved says what to do first.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SourceCheck } from "../src/pages/pipelines/SourceCheck";
import { expectAxeClean, jsonResponse, problem } from "./pageHarness";

const CHECK = "/api/v1/projects/helsinki/pipelines/stations/source-check";
const words = en.pipelines.workbench.sourceCheck;

const row = (id: number, rule: string, path: string, message: string) => ({
  id,
  at: "2026-11-15T20:00:00Z",
  record: { id: `urn:ngsi-ld:BikeStation:s-${id}`, type: "BikeStation" },
  rule,
  path,
  message,
});

const ANSWER = {
  records: 2400,
  accepted: 2310,
  rejected: 90,
  complete: true,
  errors: [],
  groups: [
    { rule: "sh:datatype", path: "capacity", count: 72, fix: "mapping", examples: [row(4, "sh:datatype", "capacity", "capacity is not an integer")] },
    { rule: "sh:in", path: "status", count: 18, fix: "mapping", allowed: ["working", "closed"], examples: [row(9, "sh:in", "status", "status is not one of the values the model lists")] },
  ],
  rows: [row(4, "sh:datatype", "capacity", "capacity is not an integer"), row(9, "sh:in", "status", "status is not one of the values the model lists")],
};

function renderCheck(answer: () => Response, { saved = true }: { saved?: boolean } = {}) {
  const pipeline = saved ? "stations" : undefined;
  const seen: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
      seen.push(`${request.method} ${new URL(request.url).pathname}`);
      return Promise.resolve(answer());
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const rendered = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <SourceCheck project="helsinki" pipeline={pipeline} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { ...rendered, seen };
}

describe("the whole-source check", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("counts_every_record_by_rule_with_its_fix_and_lists_the_first_rejected", async () => {
    const { container, seen } = renderCheck(() => jsonResponse(ANSWER));
    const section = screen.getByRole("region", { name: words.title });
    expect(within(section).getByText(words.hint)).toBeInTheDocument();
    await userEvent.click(within(section).getByRole("button", { name: words.run }));
    expect(await within(section).findByText("2,400 records: 2,310 accepted, 90 rejected")).toBeInTheDocument();
    expect(seen).toEqual([`POST ${CHECK}`]);
    expect(within(section).getByText(words.complete)).toBeInTheDocument();
    const groups = within(section).getByRole("region", { name: en.pipelines.rejected.groups.title });
    expect(within(groups).getAllByRole("listitem")).toHaveLength(2);
    expect(within(groups).getByText("72 records")).toBeInTheDocument();
    expect(within(groups).queryByRole("button", { name: en.pipelines.rejected.groups.export })).toBeNull();
    await userEvent.click(within(section).getByText("First 2 rejected records"));
    expect(within(section).getByText("Record 9")).toBeInTheDocument();
    expect(within(section).getByRole("button", { name: words.again })).toBeEnabled();
    await expectAxeClean(container);
  });

  it("shows_the_portals_words_when_the_check_is_refused", async () => {
    const detail = "a pipeline test is already running in 'helsinki'; wait for it";
    renderCheck(() => problem(409, detail));
    await userEvent.click(screen.getByRole("button", { name: words.run }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(words.failed);
    expect(alert).toHaveTextContent(detail);
  });

  it("says_a_partial_read_and_its_reason", async () => {
    const partial = { ...ANSWER, complete: false, errors: ["the check stopped at 60 s or 50000 records, so the counts cover the part of the source read by then"] };
    renderCheck(() => jsonResponse(partial));
    await userEvent.click(screen.getByRole("button", { name: words.run }));
    expect(await screen.findByText(words.partial)).toBeInTheDocument();
    expect(screen.getByText(partial.errors[0])).toBeInTheDocument();
  });

  it("asks_for_a_saved_pipeline_before_it_runs", () => {
    const { seen } = renderCheck(() => jsonResponse(ANSWER), { saved: false });
    expect(screen.getByRole("button", { name: words.run })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getAllByText(words.unsaved).length).toBeGreaterThan(0);
    expect(seen).toEqual([]);
  });
});
