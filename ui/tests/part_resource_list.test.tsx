/**
 * T-1806: the shared list table against the UI contract (UI-15, UI-16, UI-11).
 *
 * `ResourceList` draws the four states of every list page — loading, failed, empty, rows — so
 * this is where they are held to the contract: axe in each state, the retry reachable and used by
 * keyboard, and the words of each state in the four locales.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/components/ui/EmptyState.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { ApiError } from "../src/api/client";
import { ResourceList } from "../src/components/ResourceList";
import { EmptyState, TableCell, TableHead, TableHeaderCell, TableRow } from "../src/components/ui";
import { expectNoRawKeys, expectNoViolations, expectTabOrder, focusables } from "./checks";

interface ListQuery {
  isPending: boolean;
  isError: boolean;
  error: unknown;
  refetch: () => unknown;
}

const idle: ListQuery = { isPending: false, isError: false, error: null, refetch: vi.fn() };

function show(query: Partial<ListQuery> = {}, count = 1) {
  return render(
    <I18nextProvider i18n={i18n}>
      <section aria-labelledby="spaces-heading">
        <h2 id="spaces-heading">Spaces</h2>
        <ResourceList
          query={{ ...idle, ...query }}
          caption="Spaces"
          head={
            <TableHead>
              <TableHeaderCell>Name</TableHeaderCell>
            </TableHead>
          }
          columns={1}
          count={count}
          empty={<EmptyState bare title="No spaces yet" />}
        >
          <TableRow>
            <TableCell primary>ovzdusie</TableCell>
          </TableRow>
        </ResourceList>
      </section>
    </I18nextProvider>,
  );
}

describe("the resource list against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it.each([
    ["while it loads", { isPending: true }, 0],
    ["with no rows", {}, 0],
    ["with rows", {}, 2],
  ])("has no axe violation %s", async (_what, query, count) => {
    const { container } = show(query as Partial<ListQuery>, count as number);
    await expectNoViolations(container);
  });

  it("has no axe violation when the list failed, and the retry is the one control", async () => {
    const refetch = vi.fn();
    const { container } = show({
      isError: true,
      error: new ApiError(503, "Service Unavailable", {
        type: "about:blank",
        status: 503,
        title: "Service Unavailable",
        detail: "the broker is not answering",
      }),
      refetch,
    });

    expect(screen.getByRole("alert")).toHaveTextContent("the broker is not answering");
    await expectNoViolations(container);

    const alert = screen.getByRole("alert");
    await expectTabOrder(userEvent, alert);
    expect(focusables(alert)).toHaveLength(1);
  });

  it("retries by keyboard alone, with the API's own reason on the screen", async () => {
    const user = userEvent.setup();
    const refetch = vi.fn();
    show({ isError: true, error: new Error("the network went away"), refetch });

    const retry = screen.getByRole("button", { name: i18n.t("app.error.retry") });
    retry.focus();
    await user.keyboard("{Enter}");
    expect(refetch).toHaveBeenCalledTimes(1);
  });

  it.each(SUPPORTED_LOCALES)("says what is happening in %s", async (locale) => {
    await i18n.changeLanguage(locale);

    const loading = show({ isPending: true }, 0);
    expect(screen.getByRole("status")).toHaveTextContent(i18n.t("app.loading"));
    expectNoRawKeys(loading.container);
    loading.unmount();

    const failed = show({ isError: true, error: new Error("the network went away"), refetch: vi.fn() });
    expect(screen.getByRole("alert")).toHaveTextContent(i18n.t("app.error.generic"));
    expect(
      screen.getByRole("button", { name: i18n.t("app.error.retry") }),
    ).toBeInTheDocument();
    expectNoRawKeys(failed.container);
  });
});
