/**
 * T-1805: what the project holds of each quota, against the UI contract (UI-15, UI-16, UI-11).
 *
 * The card is read, not operated, and its one moving part is the bar: a progressbar has to carry
 * the same numbers the text does, or a screen reader is told a different story from the one on
 * the screen. It was also the one part of this batch with no test at all.
 */
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { ProjectQuota } from "../src/components/ProjectQuota";
import { expectNoRawKeys, expectNoViolations } from "./checks";

const USAGE = {
  contextSpaces: { used: 3, limit: 10 },
  publicEndpoints: { used: 4, limit: 4 },
  apps: { used: 7 },
};

function stub(answer: Record<string, unknown> | number) {
  vi.stubGlobal(
    "fetch",
    vi.fn(() =>
      Promise.resolve(
        typeof answer === "number"
          ? new Response(JSON.stringify({ status: answer, title: "Not Found" }), {
              status: answer,
              headers: { "Content-Type": "application/problem+json" },
            })
          : new Response(JSON.stringify(answer), {
              status: 200,
              headers: { "Content-Type": "application/json" },
            }),
      ),
    ),
  );
}

const detail = (usage: unknown) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Project",
  metadata: { name: "helsinki" },
  spec: {},
  status: { usage },
});

function show() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ProjectQuota project="helsinki" />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the project quota card against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("gives the bar the same numbers the text says, and has no axe violation", async () => {
    stub(detail(USAGE));
    const { container } = show();

    expect(await screen.findByText("3 of 10")).toBeInTheDocument();
    const spaces = screen.getByRole("progressbar", { name: "Context Spaces" });
    expect(spaces).toHaveAttribute("aria-valuenow", "3");
    expect(spaces).toHaveAttribute("aria-valuemin", "0");
    expect(spaces).toHaveAttribute("aria-valuemax", "10");
    // The filled part is a computed width, which is why that one style stays inline.
    expect((spaces.firstElementChild as HTMLElement).style.width).toBe("30%");
    // The bar is the second `<dd>` of the same term: the value, drawn (T-1805).
    expect(spaces.closest("dd")).not.toBeNull();

    await expectNoViolations(container);
  });

  it("says a dimension with no limit as a count, and draws no bar for it", async () => {
    stub(detail(USAGE));
    show();

    expect(await screen.findByText("7, no limit")).toBeInTheDocument();
    expect(screen.queryByRole("progressbar", { name: "Apps" })).toBeNull();
  });

  it("marks a dimension that is used up, in words as well as in colour", async () => {
    stub(detail(USAGE));
    show();

    const full = await screen.findByText("4 of 4");
    // UI-30: the tone is the second signal. The number itself is the first, and it is emphasised
    // rather than merely recoloured.
    expect(full.className).toContain("font-semibold");
    expect(screen.getByRole("progressbar", { name: "Public endpoints" })).toHaveAttribute(
      "aria-valuenow",
      "4",
    );
  });

  it.each([
    ["the project cannot be read", 404],
    ["the project reports no usage", detail({})],
  ])("draws nothing at all when %s", async (_what, answer) => {
    stub(answer as Record<string, unknown> | number);
    const { container } = show();

    await waitFor(() => expect(container).toBeEmptyDOMElement());
  });

  it.each(SUPPORTED_LOCALES)("writes the card in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    stub(detail(USAGE));
    const { container } = show();

    expect(
      await screen.findByRole("heading", { name: i18n.t("quota.title") }),
    ).toBeInTheDocument();
    expect(screen.getByText(i18n.t("quota.lead"))).toBeInTheDocument();
    expect(screen.getByText(i18n.t("quota.ofLimit", { used: 3, limit: 10 }))).toBeInTheDocument();
    expectNoRawKeys(container);
  });
});
