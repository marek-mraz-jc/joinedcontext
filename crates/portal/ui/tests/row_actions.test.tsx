/**
 * UI-15, UI-16, UI-26, UI-44 (T-2137, T-2279): the actions of one row — the obvious one in the
 * open, the rest behind one menu.
 *
 * A row used to carry every action as its own button, so five controls competed with the data
 * and the table could not be read down its columns. The keyboard and the roles are Radix's; what
 * this component adds is the rule, and the rule is what these cases hold: one action stays out,
 * an action nobody may take is not in the list at all, and an action that is merely unavailable
 * right now stays in it, disabled, saying why (UI-44).
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { RowActions } from "../src/components/ui/RowActions";
import type { RowAction } from "../src/components/ui/RowActions";
import { expectNoAxeViolations } from "./page_contract";

const more = (name: string) => en.rowActions.more.replace("{name}", `${name}`);

function show(actions: RowAction[], primary?: React.ReactNode) {
  return render(
    <I18nextProvider i18n={i18n}>
      <RowActions label="air-quality" primary={primary} actions={actions} />
    </I18nextProvider>,
  );
}

const open = () => userEvent.click(screen.getByRole("button", { name: more("air-quality") }));

describe("the actions of a row", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("shows the row's obvious action and hides the rest behind one menu", async () => {
    show([{ key: "delete", label: "Delete the endpoint", onSelect: vi.fn() }], <button>Open</button>);
    expect(screen.getByRole("button", { name: "Open" })).toBeInTheDocument();
    expect(screen.queryByText("Delete the endpoint")).not.toBeInTheDocument();
    await open();
    expect(await screen.findByRole("menuitem", { name: /Delete the endpoint/ })).toBeInTheDocument();
  });

  it("names the menu after the row, so two rows are two different buttons", () => {
    show([{ key: "delete", label: "Delete", onSelect: vi.fn() }]);
    expect(screen.getByRole("button", { name: more("air-quality") })).toBeInTheDocument();
  });

  it("renders no menu at all when the row has nothing behind one", () => {
    show([], <button>Open</button>);
    expect(screen.getByRole("button", { name: "Open" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: more("air-quality") })).not.toBeInTheDocument();
  });

  it("runs the action a person chooses", async () => {
    const onSelect = vi.fn();
    show([{ key: "copy", label: "Work on a copy", onSelect }]);
    await open();
    await userEvent.click(await screen.findByRole("menuitem", { name: /Work on a copy/ }));
    expect(onSelect).toHaveBeenCalledTimes(1);
  });

  it("keeps an unavailable action in the list, disabled, and says why (UI-44)", async () => {
    const onSelect = vi.fn();
    show([
      {
        key: "delete",
        label: "Delete the endpoint",
        tone: "danger",
        onSelect,
        disabledReason: "Two applications are served from it.",
      },
    ]);
    await open();
    const item = await screen.findByRole("menuitem", { name: /Delete the endpoint/ });
    expect(item).toHaveAttribute("aria-disabled", "true");
    // The reason is readable, not guessable from a missing line.
    expect(item).toHaveTextContent("Two applications are served from it.");
    await userEvent.click(item);
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("opens and chooses from the keyboard alone", async () => {
    const onSelect = vi.fn();
    show([{ key: "edit", label: "Edit the endpoint", onSelect }]);
    await userEvent.tab();
    expect(screen.getByRole("button", { name: more("air-quality") })).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    const item = await screen.findByRole("menuitem", { name: /Edit the endpoint/ });
    expect(item).toBeInTheDocument();
    await userEvent.keyboard("{Enter}");
    expect(onSelect).toHaveBeenCalledTimes(1);
  });

  it("closes on Escape without running anything", async () => {
    const onSelect = vi.fn();
    show([{ key: "edit", label: "Edit the endpoint", onSelect }]);
    await open();
    expect(await screen.findByRole("menuitem", { name: /Edit the endpoint/ })).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("menuitem")).not.toBeInTheDocument();
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("has no axe violation, closed and open", async () => {
    const { container } = show(
      [
        { key: "edit", label: "Edit the endpoint", onSelect: vi.fn() },
        { key: "delete", label: "Delete the endpoint", tone: "danger", onSelect: vi.fn(), disabledReason: "In use." },
      ],
      <button>Open</button>,
    );
    await expectNoAxeViolations(container);
    await open();
    await screen.findByRole("menuitem", { name: /Edit the endpoint/ });
    await expectNoAxeViolations(document.body);
  });

  it("opens a link item in a new tab without leaking the opener, and separates groups (T-2618)", async () => {
    const { baseElement } = show([
      { key: "source", label: "Source", href: "https://forge.example/r", separatorAfter: true },
      { key: "later", label: "Not yet", href: "https://forge.example/x", disabledReason: "No run yet." },
      { key: "delete", label: "Delete", onSelect: vi.fn() },
    ]);
    await open();
    const link = await screen.findByRole("menuitem", { name: "Source" });
    expect(link.tagName).toBe("A");
    expect(link).toHaveAttribute("href", "https://forge.example/r");
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noreferrer noopener");
    // A disabled link is no link: nothing to follow, and it says why.
    const later = screen.getByRole("menuitem", { name: /^Not yet/ });
    expect(later.tagName).not.toBe("A");
    expect(later).toHaveAttribute("aria-disabled", "true");
    expect(baseElement.querySelectorAll('[role="separator"]')).toHaveLength(1);
  });
});
