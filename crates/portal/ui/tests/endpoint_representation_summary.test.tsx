/**
 * T-4989: an endpoint list row names what the endpoint serves in one line, the three main surfaces
 * and "+N more", instead of a chip and a link per surface. The rest is shown on hover and on
 * keyboard focus and is always in the document for a screen reader.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { RepresentationSummary } from "../src/components/endpoints/links";

function wrap(node: React.ReactNode) {
  return render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);
}

describe("RepresentationSummary", () => {
  it("names the main surfaces first and counts the rest, without a link", async () => {
    await i18n.changeLanguage("en");
    const { container } = wrap(
      <RepresentationSummary served={["csv", "odata", "ogc-features", "websocket", "ngsi-ld", "mcp"]} />,
    );
    expect(screen.getByText("NGSI-LD, MCP, OGC Features")).toBeInTheDocument();
    expect(screen.getByText(en.endpoints.field.more.replace("{count}", "3"))).toBeInTheDocument();
    expect(screen.getByRole("tooltip", { hidden: true })).toHaveTextContent("CSV, OData, WebSocket");
    expect(container.querySelector("a")).toBeNull();
  });

  it("opens the rest on keyboard focus and closes it on Escape", async () => {
    await i18n.changeLanguage("en");
    wrap(<RepresentationSummary served={["ngsi-ld", "mcp", "ogc-features", "sta", "ical"]} />);
    const more = screen.getByText(en.endpoints.field.more.replace("{count}", "2"));
    const tip = screen.getByRole("tooltip", { hidden: true });
    expect(more).toHaveAttribute("aria-describedby", tip.id);
    expect(tip).toHaveClass("sr-only");
    await userEvent.tab();
    expect(more).toHaveFocus();
    expect(tip).not.toHaveClass("sr-only");
    expect(tip).toHaveTextContent("SensorThings, iCal");
    await userEvent.keyboard("{Escape}");
    expect(tip).toHaveClass("sr-only");
  });

  it("has no counter for three or fewer, and says so when there is none", async () => {
    await i18n.changeLanguage("en");
    const { rerender } = wrap(<RepresentationSummary served={["ngsi-ld", "mcp"]} />);
    expect(screen.getByText("NGSI-LD, MCP")).toBeInTheDocument();
    expect(screen.queryByRole("tooltip", { hidden: true })).toBeNull();
    rerender(
      <I18nextProvider i18n={i18n}>
        <RepresentationSummary served={[]} />
      </I18nextProvider>,
    );
    expect(screen.getByText(en.endpoints.field.noRepresentations)).toBeInTheDocument();
  });
});
