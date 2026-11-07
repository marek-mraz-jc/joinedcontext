/**
 * UI-16 (T-2747): a session that ends under an open page asks the person to sign in again
 * without leaving the page, so what they typed is still there when they come back.
 */
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { announceSessionEnded, clearSessionEnded } from "../src/api/sessionEnded";
import { SessionEndedDialog } from "../src/components/SessionEndedDialog";
import { expectNoAxeViolations, inEveryLocale } from "./page_contract";

const LOGIN = "/login?redirect_to=%2Fprojects%2Fhelsinki%2Fendpoints";

function mount() {
  const client = new QueryClient();
  const invalidate = vi.spyOn(client, "invalidateQueries");
  const view = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <input aria-label="Draft" defaultValue="" />
        <SessionEndedDialog />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { view, invalidate };
}

describe("the session-ended dialog", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    act(() => clearSessionEnded());
    vi.restoreAllMocks();
  });

  it("stays away until a session ends", () => {
    mount();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("opens the login in a new tab, keeps the page, and continues once signed in", async () => {
    const answers = [new Response(null, { status: 401 }), new Response("{}", { status: 200 })];
    const fetch = vi.spyOn(globalThis, "fetch").mockImplementation(async () => answers.shift() as Response);
    const { invalidate } = mount();
    const draft = screen.getByLabelText("Draft");
    await userEvent.type(draft, "half a manifest");

    act(() => announceSessionEnded(LOGIN));
    expect(await screen.findByRole("dialog", { name: en.app.session.title })).toBeInTheDocument();
    const signIn = screen.getByRole("link", { name: en.app.session.signIn });
    expect(signIn).toHaveAttribute("href", LOGIN);
    expect(signIn).toHaveAttribute("target", "_blank");
    expect(signIn).toHaveAttribute("rel", "noopener noreferrer");
    await userEvent.click(signIn);

    // Not signed in yet: the dialog says so and stays.
    await userEvent.click(screen.getByRole("button", { name: en.app.session.continue }));
    expect(await screen.findByRole("alert")).toHaveTextContent(en.app.session.notYet);

    await userEvent.click(screen.getByRole("button", { name: en.app.session.continue }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    // Asked through the typed client (UI-07), both times.
    const asked = fetch.mock.calls.map(([request]) => new URL((request as Request).url).pathname);
    expect(asked).toEqual(["/api/v1/auth/me", "/api/v1/auth/me"]);
    expect(invalidate).toHaveBeenCalled();
    expect(screen.getByLabelText("Draft")).toHaveValue("half a manifest");
  });

  it("says what signing in here costs before it is chosen", async () => {
    mount();
    act(() => announceSessionEnded(LOGIN));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent(en.app.session.hereCost);
    expect(screen.getByRole("button", { name: en.app.session.here })).toBeInTheDocument();
  });

  it("closes on Escape and is announced with its title", async () => {
    mount();
    act(() => announceSessionEnded(LOGIN));
    await screen.findByRole("dialog", { name: en.app.session.title });
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("has no axe violation and reads in every locale", async () => {
    mount();
    act(() => announceSessionEnded(LOGIN));
    const dialog = await screen.findByRole("dialog");
    await expectNoAxeViolations(dialog);
    await inEveryLocale(async (locale) => {
      for (const key of Object.keys(en.app.session)) {
        expect(i18n.t(`app.session.${key}`), `${locale} ${key}`).not.toMatch(/^app\./);
      }
    });
  });
});

describe("an ended session with nobody signed in", () => {
  it("is not announced: an anonymous visitor's 401 leads to the login page, not to this dialog", async () => {
    announceSessionEnded(LOGIN);
    mount();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
