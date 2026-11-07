import { screen } from "@testing-library/react";

/**
 * A bigger form as it opens under the router: a page of its own in the list's place, named by
 * its title, not a dialog (T-2474, UI-27). A component rendered outside the router still opens
 * its form as a dialog, and the confirms stay dialogs everywhere.
 */
export function findFormPage(name?: string | RegExp): Promise<HTMLElement> {
  return name === undefined ? screen.findByTestId("form-page") : screen.findByRole("region", { name });
}

export function queryFormPage(): HTMLElement | null {
  return screen.queryByTestId("form-page");
}
