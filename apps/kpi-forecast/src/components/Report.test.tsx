import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ServerProblem } from "../server";
import { memoryServer } from "../test-server";
import { recentMonths, Report } from "./Report";

const went: string[] = [];
vi.mock("../go", () => ({ go: (url: string) => went.push(url) }));

const NOW = Date.UTC(2026, 0, 15);

beforeEach(() => {
  went.length = 0;
});

// T-3350: a month's forecasts against the readings, as CSV from the server.
describe("Report", () => {
  it("offers this month and the two before, across a new year", () => {
    expect(recentMonths(NOW)).toEqual(["2026-01", "2025-12", "2025-11"]);
  });

  it("downloads the chosen month's report", async () => {
    const api = memoryServer();
    render(<Report lang="en" api={api} now={NOW} />);
    fireEvent.change(screen.getByLabelText("Month"), { target: { value: "2025-12" } });
    fireEvent.click(screen.getByRole("button", { name: "Download forecasts against readings (CSV)" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("The CSV file is downloading."));
    expect(api.reportUrl).toHaveBeenCalledWith("2025-12");
    expect(went).toEqual(["https://store.example/apps/s1/x/reports/2025-12.csv?X-Amz-Signature=x"]);
  });

  it("says in the reader's language why the server refused", async () => {
    const api = memoryServer();
    api.reportUrl = vi.fn().mockRejectedValue(new ServerProblem(400, "a report covers a month of the last 90 days, the history the App may read"));
    render(<Report lang="fi" api={api} now={NOW} />);
    fireEvent.change(screen.getByLabelText("Kuukausi"), { target: { value: "2025-11" } });
    fireEvent.click(screen.getByRole("button", { name: "Lataa ennusteet ja toteutuma (CSV)" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Palvelin ei hyväksynyt pyyntöä: a report covers a month of the last 90 days"));
    expect(went).toEqual([]);
  });
});
