/**
 * T-5567, PF-128: a decision refused for want of a second factor asks the person to sign in
 * again with their one-time code in a new tab; the page and what they typed stay.
 */
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { api } from "../src/api/client";
import { clearStepUp, stepUpUrl } from "../src/api/stepUp";
import { StepUpDialog } from "../src/components/StepUpDialog";
import { expectNoAxeViolations, inEveryLocale } from "./page_contract";

const REFUSAL = {
  type: "https://joinedcontext.com/errors/second-factor-required",
  title: "Second Factor Required",
  status: 403,
  detail: "This decision needs a second factor in this session.",
};

function json(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/problem+json" } });
}

function mount() {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <I18nextProvider i18n={i18n}>
        <input aria-label="Reason" defaultValue="" />
        <StepUpDialog />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

async function approve(): Promise<number> {
  const { response } = await api.POST("/api/v1/projects/{project}/changes/{id}/approve", {
    params: { path: { project: "helsinki", id: "c-1" } },
  });
  return response.status;
}

describe("the step-up dialog", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.replaceState(null, "", "/projects/helsinki/changes/c-1");
  });
  afterEach(() => {
    act(() => clearStepUp());
    vi.restoreAllMocks();
  });

  it("opens on a refused decision, sends to the step-up login and continues once the code arrived", async () => {
    const answers = [
      json(REFUSAL, 403),
      json({ subject: "s", username: "jana", second_factor: false, front: "edge" }, 200),
      json({ subject: "s", username: "jana", second_factor: true, front: "edge" }, 200),
    ];
    const fetch = vi.spyOn(globalThis, "fetch").mockImplementation(async () => answers.shift() as Response);
    mount();
    await userEvent.type(screen.getByLabelText("Reason"), "checked twice");

    expect(await approve()).toBe(403);
    expect(await screen.findByRole("dialog", { name: en.app.stepUp.title })).toBeInTheDocument();
    const signIn = screen.getByRole("link", { name: en.app.stepUp.signIn });
    expect(signIn).toHaveAttribute("href", stepUpUrl("/projects/helsinki/changes/c-1"));
    expect(signIn.getAttribute("href")).toBe(
      "/api/v1/auth/login?step_up=true&redirect_to=%2Fprojects%2Fhelsinki%2Fchanges%2Fc-1",
    );
    expect(signIn).toHaveAttribute("target", "_blank");
    expect(signIn).toHaveAttribute("rel", "noopener noreferrer");
    await userEvent.click(signIn);

    await userEvent.click(screen.getByRole("button", { name: en.app.stepUp.continue }));
    expect(await screen.findByRole("alert")).toHaveTextContent(en.app.stepUp.notYet);
    await userEvent.click(screen.getByRole("button", { name: en.app.stepUp.continue }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    const asked = fetch.mock.calls.map(([request]) => new URL((request as Request).url).pathname);
    expect(asked).toEqual(["/api/v1/projects/helsinki/changes/c-1/approve", "/api/v1/auth/me", "/api/v1/auth/me"]);
    expect(screen.getByLabelText("Reason")).toHaveValue("checked twice");
  });

  it("stays away for any other refusal", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async () =>
      json({ ...REFUSAL, type: "https://joinedcontext.com/errors/forbidden" }, 403),
    );
    mount();
    expect(await approve()).toBe(403);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("has no axe violation, closes on Escape and reads in every locale", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async () => json(REFUSAL, 403));
    mount();
    await approve();
    const dialog = await screen.findByRole("dialog", { name: en.app.stepUp.title });
    expect(dialog).toHaveTextContent(en.app.stepUp.how);
    await expectNoAxeViolations(dialog);
    await inEveryLocale(async (locale) => {
      for (const key of Object.keys(en.app.stepUp)) {
        expect(i18n.t(`app.stepUp.${key}`), `${locale} ${key}`).not.toMatch(/^app\./);
      }
      expect(i18n.t("problem.second-factor-required"), locale).not.toMatch(/^problem\./);
    });
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });
});
