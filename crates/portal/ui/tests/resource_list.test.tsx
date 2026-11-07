import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ApiError } from "../src/api/client";
import { ResourceList } from "../src/components/ResourceList";
import { EmptyState, TableCell, TableHead, TableHeaderCell, TableRow } from "../src/components/ui";

interface ListQuery {
  isPending: boolean;
  isError: boolean;
  error: unknown;
  refetch: () => unknown;
}

const idle: ListQuery = { isPending: false, isError: false, error: null, refetch: vi.fn() };

function show(query: Partial<ListQuery>, count = 0) {
  return render(
    <I18nextProvider i18n={i18n}>
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
        empty={<EmptyState bare title="No spaces yet" action={<button type="button">New space</button>} />}
      >
        <TableRow>
          <TableCell>ovzdusie</TableCell>
        </TableRow>
      </ResourceList>
    </I18nextProvider>,
  );
}

describe("ResourceList (T-1382)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("says it is loading and draws no rows yet", () => {
    show({ isPending: true });
    expect(screen.getByRole("table", { name: "Spaces" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(en.app.loading);
    expect(screen.queryByText("ovzdusie")).toBeNull();
    expect(screen.queryByText("No spaces yet")).toBeNull();
  });

  it("puts the API's own reason where the table would be, and retries", async () => {
    const refetch = vi.fn();
    show({
      isError: true,
      refetch,
      error: new ApiError(503, "Service Unavailable", { type: "about:blank", status: 503, title: "Service Unavailable", detail: "The forge is down." }),
    });
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.getByRole("alert")).toHaveTextContent("The forge is down.");
    await userEvent.click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(refetch).toHaveBeenCalledOnce();
  });

  it("says something generic for an error that is not the API's", () => {
    show({ isError: true, error: new TypeError("Failed to fetch") });
    expect(screen.getByRole("alert")).toHaveTextContent(en.app.error.generic);
  });

  it("shows the empty state with its action inside the table when the list holds nothing", () => {
    show({}, 0);
    const table = screen.getByRole("table", { name: "Spaces" });
    expect(table).toContainElement(screen.getByText("No spaces yet"));
    expect(screen.getByRole("button", { name: "New space" })).toBeInTheDocument();
    expect(screen.queryByText("ovzdusie")).toBeNull();
  });

  it("draws the rows otherwise", () => {
    show({}, 1);
    expect(screen.getByText("ovzdusie")).toBeInTheDocument();
    expect(screen.queryByText("No spaces yet")).toBeNull();
  });
});
