/** T-6136: the broker editor's chip list input, rendered on its own (the connected-clients cell went with T-6133: no page rendered it). */
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { ChipInput } from "../src/pages/mqttbrokers/BrokerEditor";

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(cleanup);

describe("chip list input (T-6103)", () => {
  it("adds a typed value on Enter, refuses a duplicate and removes a chip", () => {
    const onChange = vi.fn();
    render(
      <ChipInput label="Topics" help="One topic per entry" values={["a/b"]} disabled={false} problem={() => undefined} onChange={onChange} />,
    );
    const input = screen.getByRole("textbox", { name: "Topics" });
    fireEvent.change(input, { target: { value: "a/b" } });
    expect(input.getAttribute("aria-invalid")).toBe("true");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: " c/d " } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onChange).toHaveBeenLastCalledWith(["a/b", "c/d"]);
    fireEvent.click(screen.getByRole("button", { name: /remove/i }));
    expect(onChange).toHaveBeenLastCalledWith([]);
  });

  it("shows the values without an input when read-only", () => {
    render(<ChipInput label="Topics" help="h" values={["a/b"]} disabled problem={() => undefined} onChange={vi.fn()} />);
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.getByText("a/b")).toBeTruthy();
  });
});
