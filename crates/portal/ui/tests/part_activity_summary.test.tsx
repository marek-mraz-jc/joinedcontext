/**
 * T-1802: the last hour as five numbers, against the UI contract (UI-15, UI-16, UI-31).
 *
 * The strip is read, not operated: what matters is that each number is tied to the word for it,
 * that the pending strip says so rather than showing a zero nobody measured, and that a failed
 * read leaves the page alone instead of putting an error where a number belongs.
 */
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { ActivitySummary, BUCKETS, summarise } from "../src/components/ActivitySummary";
import type { ActivityEvent } from "../src/api/activity";
import { expectNoRawKeys, expectNoViolations } from "./checks";

const EVENTS = [
  { kind: "pipeline.throughput", details: { count: 1200 } },
  { kind: "endpoint.traffic", details: { count: 37 } },
  { kind: "access.denied" },
  { kind: "access.denied" },
  { kind: "change.merged" },
] as unknown as ActivityEvent[];

function stub(answer: { items?: unknown[] } | number) {
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      Promise.resolve(
        typeof answer === "number"
          ? new Response(JSON.stringify({ status: answer, title: "Server Error" }), {
              status: answer,
              headers: { "Content-Type": "application/problem+json" },
            })
          : new Response(
              JSON.stringify({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", ...answer }),
              { status: 200, headers: { "Content-Type": "application/json" } },
            ),
      ),
    ),
  );
}

function show() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ActivitySummary project="helsinki" />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the activity summary against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("counts what an event reports and one for what it does not", () => {
    // The unit behind the strip: `details.count` when it is there, one otherwise (UI-31).
    expect(summarise(EVENTS)).toEqual({
      messages: 1200,
      changes: 1,
      requests: 37,
      forwards: 0,
      denials: 2,
    });
    expect(summarise([])).toEqual({
      messages: 0,
      changes: 0,
      requests: 0,
      forwards: 0,
      denials: 0,
    });
  });

  it("ties every number to the word for it, and has no axe violation", async () => {
    stub({ items: EVENTS });
    const { container } = show();

    for (const bucket of BUCKETS) {
      const term = await screen.findByText(i18n.t(`activity.bucket.${bucket.key}`));
      const number = term.parentElement?.querySelector("dd");
      expect(number, `${bucket.key} has a number beside its word`).not.toBeNull();
      // And it is bigger than the word: a size the theme defines, not `text-h3`, which the
      // theme never had and Tailwind emits nothing for — the counters read as body text beside
      // their own captions until T-2422.
      expect(number?.className, bucket.key).toMatch(/\btext-(title|display)\b/);
    }
    await waitFor(() => expect(screen.getByText("1,200")).toBeInTheDocument());
    await expectNoViolations(container);
  });

  it("shows no number it has not read yet", () => {
    stub({ items: EVENTS });
    show();

    // A zero would be a measurement; a dash is the truth while the hour is still being read.
    expect(screen.getAllByText("—")).toHaveLength(BUCKETS.length);
  });

  it("leaves the page alone when the hour cannot be read", async () => {
    stub(500);
    const { container } = show();

    await waitFor(() => expect(container).toBeEmptyDOMElement());
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it.each(SUPPORTED_LOCALES)("names the five buckets in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    stub({ items: EVENTS });
    const { container } = show();

    expect(
      await screen.findByRole("heading", { name: i18n.t("activity.lastHour") }),
    ).toBeInTheDocument();
    for (const bucket of BUCKETS) {
      expect(
        screen.getByText(i18n.t(`activity.bucket.${bucket.key}`)),
        `${bucket.key} in ${locale}`,
      ).toBeInTheDocument();
    }
    expectNoRawKeys(container);
  });
});
