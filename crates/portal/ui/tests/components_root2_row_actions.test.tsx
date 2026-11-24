/** The Save as and Work on a copy items of a manifest row's menu open their dialog, and closing it leaves the row as it was. */
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ResourceRowActions } from "../src/components/ResourceRowActions";
import { json, renderPage } from "./page_contract";

const TARGET = { project: "helsinki", kind: "Endpoint", plural: "endpoints", name: "air-quality", label: "Air quality" };
const more = en.rowActions.more.replace("{name}", "Air quality");

describe("row menu: Save as and Work on a copy", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => vi.restoreAllMocks());

  it.each([
    ["Save as", en.saveAs.button],
    ["Work on a copy", en.workspaces.open.action],
  ])("%s opens its dialog and Escape closes it", async (_name, item) => {
    renderPage(<ResourceRowActions project="helsinki" target={TARGET} />, {
      answer: (url) =>
        url.pathname.endsWith("/permissions/me")
          ? json({ grants: [{ rule: { kinds: ["Endpoint"], verbs: ["propose", "delete"] } }], bootstrap: false })
          : undefined,
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: more }));
    await user.click(await screen.findByRole("menuitem", { name: item }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    // the menu is still there for the next action
    expect(screen.getByRole("button", { name: more })).toBeInTheDocument();
  });

  it("Edit without a page editor opens the edit dialog; the row's own actions come first, a line after the last", async () => {
    const mine = vi.fn();
    renderPage(<ResourceRowActions project="helsinki" target={TARGET} extra={[{ key: "a", label: "First own" }, { key: "b", label: "Second own", onSelect: mine }]} />, {
      answer: (url) =>
        url.pathname.endsWith("/permissions/me")
          ? json({ grants: [{ rule: { kinds: ["Endpoint"], verbs: ["propose", "delete"] } }], bootstrap: false })
          : undefined,
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: more }));
    const labels = (await screen.findAllByRole("menuitem")).map((item) => item.textContent);
    expect(labels.slice(0, 2)).toEqual(["First own", "Second own"]);
    expect(screen.getAllByRole("separator")).toHaveLength(1);
    await user.click(screen.getByRole("menuitem", { name: en.resourceEdit.button }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    expect(mine).not.toHaveBeenCalled();
  });
});
