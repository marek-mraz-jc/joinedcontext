import { createContext, Fragment, useContext } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from "./Menu";
import { Button } from "./Button";

/** One thing a row can do, as the menu shows it. */
export interface RowAction {
  key: string;
  label: string;
  /** What choosing it does; an item with `href` is a link instead and needs none. */
  onSelect?: () => void;
  /** A place elsewhere (the forge): the item is a link that opens in a new tab. */
  href?: string;
  tone?: "default" | "danger";
  /** A line after this item, between one block of the menu and the next. */
  separatorAfter?: boolean;
  /**
   * Why this action cannot be taken right now (UI-44). The item stays in the menu, disabled, and
   * says the reason — a person whose role is too narrow has to be able to read that, not guess it
   * from a missing line.
   */
  disabledReason?: string;
}

/** Set by a grid row: its details panel, offered as the menu's first item, so a row ends in one button. */
export const RowDetails = createContext<{ label: string; onSelect: () => void } | null>(null);

/**
 * The actions of one row: the obvious one in the open, the rest behind one menu at the end (T-2279,
 * UI-26).
 *
 * A row used to carry every action as its own button — open, edit, save as, work on a copy, delete —
 * so five controls competed with the data and the table could not be read down its columns. The menu
 * is Radix's, so the keyboard, the roles and Escape come from it; what this adds is the rule: one
 * action stays out, the others go in, and an action a person may not take is simply not in the list.
 *
 * A dialog must NOT be rendered inside the menu: the menu unmounts when it closes and would take the
 * dialog with it. Every action here only flips the row's own state, and the row renders the dialogs
 * beside the menu, which is what the `trigger={false}` of each action component is for.
 */
export function RowActions({
  label,
  primary,
  actions,
}: {
  /** What this row is, for the menu button's accessible name. */
  label: string;
  /** The action that stays in the open, if the row has an obvious one. */
  primary?: ReactNode;
  actions: RowAction[];
}): JSX.Element {
  const { t } = useTranslation();
  const details = useContext(RowDetails);
  if (details) {
    actions = [{ key: "details", ...details, separatorAfter: actions.length > 0 }, ...actions];
  }
  if (actions.length === 0) {
    return <>{primary}</>;
  }
  return (
    // The buttons may shrink here (`!`: over their own `shrink-0`), so a label wraps rather than spilling: a `shrink-0` button whose
    // label could wrap gave its table cell one word's width, and at 375 px "Pozrieť obsah" lay
    // over the row's state badge (T-4265, photo audit T-4160 #0).
    <div className="flex items-center justify-end gap-1.5 [&>*]:shrink!">
      {primary}
      <Menu>
        <MenuTrigger asChild>
          <Button size="sm" variant="secondary" aria-label={t("rowActions.more", { name: label })}>
            ⋯
          </Button>
        </MenuTrigger>
        <MenuContent align="end">
          {actions.map((action) => (
            <Fragment key={action.key}>
              {action.href && !action.disabledReason ? (
                <MenuItem asChild tone={action.tone}>
                  <a href={action.href} target="_blank" rel="noreferrer noopener">
                    {action.label}
                  </a>
                </MenuItem>
              ) : (
                <MenuItem
                  tone={action.tone}
                  disabled={Boolean(action.disabledReason)}
                  title={action.disabledReason}
                  aria-disabled={action.disabledReason ? "true" : undefined}
                  onSelect={action.disabledReason ? undefined : action.onSelect}
                >
                  {action.label}
                  {action.disabledReason ? (
                    <span className="sr-only"> — {action.disabledReason}</span>
                  ) : null}
                </MenuItem>
              )}
              {action.separatorAfter ? <MenuSeparator /> : null}
            </Fragment>
          ))}
        </MenuContent>
      </Menu>
    </div>
  );
}
