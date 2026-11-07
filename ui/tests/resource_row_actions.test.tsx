/**
 * UI-15, UI-16, UI-26, UI-44 (T-2137, T-2287): the four actions every manifest row carries.
 *
 * `RowActions` owns the menu and its keyboard; this component owns what goes in it and who may
 * take it. Those are the cases: the four are behind the one menu with the page's own action left
 * in the open, an action the caller's role does not permit stays listed and disabled with the
 * sentence that names the verb and the kind (never silently missing, and never enabled to fail at
 * the API), and Remove asks before anything is proposed.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ResourceRowActions } from "../src/components/ResourceRowActions";
import { expectNoAxeViolations, json, renderPage } from "./page_contract";

const TARGET = {
  project: "helsinki",
  kind: "Endpoint",
  plural: "endpoints",
  name: "air-quality",
  label: "Air quality",
};

const more = en.rowActions.more.replace("{name}", "Air quality");

/** The sentence a disabled action carries, as i18next renders it. */
const denied = (verb: string, kind: string) => i18n.t("permissions.denied", { verb, kind });

/** A permissions document that grants exactly `verbs` on `Endpoint`. */
function effective(verbs: string[]) {
  return { grants: [{ rule: { kinds: ["Endpoint"], verbs } }], bootstrap: false };
}

function show(grants: unknown, props: Record<string, unknown> = {}) {
  return renderPage(<ResourceRowActions project="helsinki" target={TARGET} {...props} />, {
    answer: (url) => (url.pathname.endsWith("/permissions/me") ? json(grants) : undefined),
  });
}

const openMenu = async () => {
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: more }));
  return user;
};

afterEach(() => vi.restoreAllMocks());

describe("the actions of a manifest row", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("keeps the page's own action in the open and the four behind one menu", async () => {
    show(effective(["propose", "delete"]), { primary: <button>Open</button> });
    expect(await screen.findByRole("button", { name: "Open" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem")).not.toBeInTheDocument();

    await openMenu();
    const items = await screen.findAllByRole("menuitem");
    expect(items.map((item) => item.textContent)).toEqual([
      en.resourceEdit.button,
      en.saveAs.button,
      en.workspaces.open.action,
      en.resourceDelete.button,
    ]);
  });

  it("disables what the role may not do and says why, rather than hiding it", async () => {
    // A reader: no propose, no delete. "Work on a copy" is a workspace of one's own and needs
    // neither, so it stays available — a person is not left with an empty menu.
    show(effective([]));
    await openMenu();

    const edit = await screen.findByRole("menuitem", { name: new RegExp(en.resourceEdit.button) });
    expect(edit).toHaveAttribute("aria-disabled", "true");
    expect(within(edit).getByText(denied("propose", "Endpoint"), { exact: false })).toBeInTheDocument();

    const remove = screen.getByRole("menuitem", { name: new RegExp(en.resourceDelete.button) });
    expect(remove).toHaveAttribute("aria-disabled", "true");
    expect(within(remove).getByText(denied("delete", "Endpoint"), { exact: false })).toBeInTheDocument();

    expect(
      screen.getByRole("menuitem", { name: en.workspaces.open.action }),
    ).not.toHaveAttribute("aria-disabled");
  });

  it("opens nothing when a disabled action is chosen", async () => {
    show(effective([]));
    const user = await openMenu();
    await user.click(await screen.findByRole("menuitem", { name: new RegExp(en.resourceDelete.button) }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("asks before it proposes a removal", async () => {
    show(effective(["propose", "delete"]));
    const user = await openMenu();
    await user.click(await screen.findByRole("menuitem", { name: en.resourceDelete.button }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(en.resourceDelete.title.replace("{name}", "Air quality"))).toBeInTheDocument();
    // Nothing is removed by opening it: the name has to be typed back first. `aria-disabled`,
    // not `disabled`: a refusal that carries a reason keeps its button in the tab order so the
    // reason can be reached and read (T-1830, UI-44).
    expect(within(dialog).getByRole("button", { name: en.resourceDelete.propose })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

  it("lets a page with its own editor handle Edit itself", async () => {
    const onEdit = vi.fn();
    show(effective(["propose", "delete"]), { onEdit });
    const user = await openMenu();
    await user.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
    expect(onEdit).toHaveBeenCalledTimes(1);
    // The page edits in place, so no edit dialog is mounted beside the menu.
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("has no axe violation, closed and open", async () => {
    const view = show(effective(["propose", "delete"]), { primary: <button>Open</button> });
    await screen.findByRole("button", { name: more });
    await expectNoAxeViolations(view.container);
    await openMenu();
    await screen.findAllByRole("menuitem");
    await expectNoAxeViolations(document.body);
  });
});
