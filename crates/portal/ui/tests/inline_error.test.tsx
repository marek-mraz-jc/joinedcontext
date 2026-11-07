/**
 * T-3281: the one error line beside the thing it is about. It is announced, it says "Error" in a
 * word and not in colour alone, and it is drawn at a Field error's size and red wherever it sits.
 */
import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { InlineError } from "../src/components/ui/InlineError";

function show(node: React.ReactNode) {
  return render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);
}

describe("InlineError (T-3281)", () => {
  it("is announced, with the tone in a word before the message", () => {
    show(<InlineError>The list could not be read.</InlineError>);
    const line = screen.getByRole("alert");
    expect(line).toHaveTextContent(`${en.app.alert.danger}The list could not be read.`);
    expect(screen.getByText(en.app.alert.danger)).toHaveClass("sr-only");
    // The glyph is for the eye only.
    expect(line.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
  });

  it("keeps its size and red whatever the caller adds, and passes the rest on", () => {
    show(
      <InlineError id="list-failed" className="mt-2">
        No answer.
      </InlineError>,
    );
    const line = screen.getByRole("alert");
    expect(line).toHaveAttribute("id", "list-failed");
    expect(line).toHaveClass("mt-2", "text-caption", "text-danger");
    expect(line).not.toHaveClass("text-danger-fg");
  });

  it("lets an address with no space wrap rather than push the page sideways", () => {
    show(<InlineError>{"https://example.org/".padEnd(300, "a")}</InlineError>);
    expect(screen.getByText(/^https:\/\/example\.org/)).toHaveClass("[overflow-wrap:anywhere]");
  });
});
