/**
 * The session-ended and step-up dialogs when the sign-in check cannot reach the server, and the
 * "sign in here instead" button of the session dialog.
 */
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { announceSessionEnded, clearSessionEnded } from "../src/api/sessionEnded";
import { clearStepUp, announceStepUp } from "../src/api/stepUp";
import { SessionEndedDialog } from "../src/components/SessionEndedDialog";
import { StepUpDialog } from "../src/components/StepUpDialog";

const LOGIN = "/login?redirect_to=%2Fprojects%2Fhelsinki%2Fendpoints";

function mount(dialog: React.ReactNode) {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <I18nextProvider i18n={i18n}>{dialog}</I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the sign-in dialogs when the check fails", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    act(() => {
      clearSessionEnded();
      clearStepUp();
    });
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("session dialog: a network error on Continue says not yet and keeps the dialog", async () => {
    const fetch = vi.spyOn(globalThis, "fetch").mockRejectedValue(new TypeError("offline"));
    mount(<SessionEndedDialog />);
    act(() => announceSessionEnded(LOGIN));
    await userEvent.click(await screen.findByRole("link", { name: en.app.session.signIn }));
    await userEvent.click(screen.getByRole("button", { name: en.app.session.continue }));
    expect(await screen.findByRole("alert")).toHaveTextContent(en.app.session.notYet);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("session dialog: 'sign in here instead' leaves the page for the login url", async () => {
    const assign = vi.fn();
    vi.stubGlobal("location", { ...window.location, assign });
    mount(<SessionEndedDialog />);
    act(() => announceSessionEnded(LOGIN));
    await userEvent.click(await screen.findByRole("button", { name: en.app.session.here }));
    expect(assign).toHaveBeenCalledWith(LOGIN);
  });

  it("step-up dialog: a network error on Continue says not yet and keeps the dialog", async () => {
    const fetch = vi.spyOn(globalThis, "fetch").mockRejectedValue(new TypeError("offline"));
    mount(<StepUpDialog />);
    act(() => announceStepUp("/api/v1/auth/login?step_up=true"));
    await userEvent.click(await screen.findByRole("link", { name: en.app.stepUp.signIn }));
    await userEvent.click(screen.getByRole("button", { name: en.app.stepUp.continue }));
    expect(await screen.findByRole("alert")).toHaveTextContent(en.app.stepUp.notYet);
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});
