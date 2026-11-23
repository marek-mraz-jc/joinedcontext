/**
 * T-7585: a form that holds its own open state (a data source's) closes when the browser's back
 * button takes its address away.
 */
import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import en from "../src/locales/en.json";
import { findFormPage, queryFormPage } from "./formPage";
import { renderRoute } from "./pageHarness";

const LIST = "/projects/helsinki/datasources";

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("the address leaves a form that kept its own state", () => {
  it("closes the form when the browser goes back to the list", async () => {
    await renderRoute({ path: LIST, doorsAsRest: true });
    await userEvent.click(await screen.findByRole("button", { name: en.datasources.add }));
    await waitFor(() => expect(window.location.pathname).toBe(`${LIST}/new`));
    await findFormPage();
    act(() => window.history.back());
    await waitFor(() => expect(window.location.pathname).toBe(LIST));
    await waitFor(() => expect(queryFormPage()).toBeNull());
  });
});
