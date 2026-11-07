// covers (T-2137, the module gate in gate_modules.test.ts): the cases in this file drive
// src/components/ui/Checkbox.tsx, src/components/ui/Tabs.tsx through the page they belong to; each was confirmed by
// making the module throw and watching this file go red.
// UI-16, UI-01 (T-1727): the shared tab list and checkbox every page uses instead of its own.
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Checkbox, Tabs, tabPanelProps } from "../src/components/ui";

type Name = "import" | "editor" | "mappings";
const TABS: { value: Name; label: string }[] = [
  { value: "import", label: "Import" },
  { value: "editor", label: "Editor" },
  { value: "mappings", label: "Mappings" },
];

function Harness({ onChange }: { onChange?: (value: Name) => void }) {
  const [tab, setTab] = useState<Name>("import");
  return (
    <>
      <Tabs
        id="models"
        label="Model views"
        tabs={TABS}
        value={tab}
        onChange={(value) => {
          setTab(value);
          onChange?.(value);
        }}
      />
      <div {...tabPanelProps("models", tab)}>panel of {tab}</div>
    </>
  );
}

describe("Tabs", () => {
  it("is one tab stop, and each tab names the panel it controls", () => {
    render(<Harness />);
    expect(screen.getByRole("tablist", { name: "Model views" })).toBeInTheDocument();
    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((tab) => tab.getAttribute("tabindex"))).toEqual(["0", "-1", "-1"]);
    expect(tabs.map((tab) => tab.getAttribute("aria-selected"))).toEqual(["true", "false", "false"]);
    // The panel is labelled by the selected tab, and that tab points back at it.
    const panel = screen.getByRole("tabpanel", { name: "Import" });
    expect(tabs[0].getAttribute("aria-controls")).toBe(panel.id);
  });

  it("moves and selects with the arrows, Home and End, wrapping at both ends", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.tab();
    expect(screen.getByRole("tab", { name: "Import" })).toHaveFocus();

    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "Editor" })).toHaveFocus();
    expect(screen.getByRole("tabpanel")).toHaveTextContent("panel of editor");

    await user.keyboard("{End}");
    expect(screen.getByRole("tab", { name: "Mappings" })).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "Import" })).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("tab", { name: "Mappings" })).toHaveFocus();
    await user.keyboard("{Home}");
    expect(screen.getByRole("tabpanel", { name: "Import" })).toBeInTheDocument();

    // Tab leaves the list rather than walking through it.
    await user.tab();
    expect(screen.getByRole("tab", { name: "Import" })).not.toHaveFocus();
    expect(document.activeElement?.getAttribute("role")).not.toBe("tab");
  });

  it("selects by click and ignores keys that are not its own", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Harness onChange={onChange} />);
    await user.click(screen.getByRole("tab", { name: "Mappings" }));
    expect(onChange).toHaveBeenLastCalledWith("mappings");
    onChange.mockClear();
    await user.keyboard("a{ArrowDown}");
    expect(onChange).not.toHaveBeenCalled();
  });

  it("renders an empty list without throwing on a key", async () => {
    const user = userEvent.setup();
    render(<Tabs id="none" label="Nothing" tabs={[]} value={"x"} onChange={() => undefined} />);
    screen.getByRole("tablist").focus();
    await user.keyboard("{ArrowRight}");
    expect(screen.queryAllByRole("tab")).toHaveLength(0);
  });
});

describe("Checkbox", () => {
  it("is named by its label and hint, and ticks by click on the text and by Space", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Checkbox label="dateObserved" hint="required" onChange={onChange} />);
    const box = screen.getByRole("checkbox", { name: /dateObserved\s*required/ });
    await user.click(screen.getByText("dateObserved"));
    expect(onChange).toHaveBeenCalledTimes(1);
    box.focus();
    await user.keyboard(" ");
    expect(onChange).toHaveBeenCalledTimes(2);
  });

  // T-2835: a caller's hint and the reason a box is refused are both read, neither drops the other.
  it("is described by its caller's hint and, when refused, by the reason too", () => {
    render(
      <>
        <Checkbox label="advanced" aria-describedby="advanced-hint" />
        <span id="advanced-hint">Also shows the rare fields.</span>
        <Checkbox label="locked" aria-describedby="locked-hint" disabled disabledReason="Only an owner may." />
        <span id="locked-hint">Keeps the source.</span>
      </>,
    );
    expect(screen.getByRole("checkbox", { name: "advanced" })).toHaveAccessibleDescription("Also shows the rare fields.");
    expect(screen.getByRole("checkbox", { name: "locked" })).toHaveAccessibleDescription(
      "Keeps the source. Only an owner may.",
    );
  });

  it("cannot be ticked when disabled", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Checkbox label="locked" disabled checked={false} onChange={onChange} />);
    await user.click(screen.getByText("locked"));
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole("checkbox", { name: "locked" })).toBeDisabled();
  });
});
