// A grid config the SDK refuses draws its findings in place of the table. The panel builds its
// config itself, so the refusal is forced here by a parser that finds fault with every config.
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import "../src/i18n";

vi.mock("@joinedcontext/sdk/grid", async (original) => ({
  ...(await original<typeof import("@joinedcontext/sdk/grid")>()),
  parseGridConfig: () => ({ findings: [{ path: "/columns/0/label", message: "must be a string" }, { path: "/pageSize", message: "too big" }] }),
}));

const { TablePanel } = await import("../src/components/panels/TablePanel");

describe("table panel refused config", () => {
  it("names every finding instead of drawing a table", () => {
    render(<TablePanel title="Counts" frames={[{ name: "", fields: [{ name: "n", type: "number", values: [1] }] }]} locale="en" />);
    expect(screen.getByText("must be a string; too big")).toBeInTheDocument();
    expect(screen.queryByRole("grid")).toBeNull();
  });
});
