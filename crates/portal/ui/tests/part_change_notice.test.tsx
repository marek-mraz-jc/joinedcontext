/**
 * T-1803: what a write leaves behind, against the UI contract (UI-15, UI-16, UI-48).
 *
 * `ChangeNotice` is the one thing a person sees after proposing anything, so it has to say that
 * nothing is saved yet and where the change is: announced as a status, the lane and the phase in
 * words beside their colour, and the link to the approval reachable by keyboard.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import type { Change } from "../src/api/manifest";
import { expectNoRawKeys, expectNoViolations, focusables } from "./checks";

vi.mock("@tanstack/react-router", () => ({
  Link: ({
    children,
    to,
    params,
    className,
  }: {
    children: React.ReactNode;
    to: string;
    params: Record<string, string>;
    className?: string;
  }) => (
    <a className={className} href={Object.entries(params).reduce((path, [key, value]) => path.replace(`$${key}`, value), to)}>
      {children}
    </a>
  ),
}));

const { ChangeNotice } = await import("../src/components/ChangeNotice");

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-11aa22bb", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 2 } },
} as unknown as Change;

function show(change: Change = CHANGE) {
  return render(
    <I18nextProvider i18n={i18n}>
      <ChangeNotice change={change} project="banskabystrica" />
    </I18nextProvider>,
  );
}

describe("the change notice against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("is announced as a status, not as an alert, and names the change", () => {
    show();
    const status = screen.getByRole("status");
    expect(status).toHaveTextContent("chg-11aa22bb");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("has no axe violation, and the review link is the one control", async () => {
    const { container } = show();
    await expectNoViolations(container);

    const status = screen.getByRole("status");
    expect(focusables(status)).toHaveLength(1);
    const review = screen.getByRole("link", { name: i18n.t("changes.review") });
    expect(review).toHaveAttribute("href", "/projects/banskabystrica/approvals/chg-11aa22bb");
  });

  it("reaches the approval by keyboard", async () => {
    const user = userEvent.setup();
    show();

    await user.tab();
    expect(screen.getByRole("link", { name: i18n.t("changes.review") })).toHaveFocus();
  });

  it("says the lane and the phase in words, so colour is never the only carrier", () => {
    show();
    const status = screen.getByRole("status");
    // UI-30: both badges carry text; the tone is the second signal, never the first.
    expect(status).toHaveTextContent(/yellow|Waiting|Pending/i);
    expect(status.querySelectorAll("span").length).toBeGreaterThan(2);
  });

  // PF-58: an administrator's change was approved as it was proposed; the notice says so and
  // opens the change instead of sending them to review it.
  it("says an approved-as-proposed change is applying and links to it", () => {
    show({ ...CHANGE, status: { ...CHANGE.status, phase: "Deploying" } } as Change);
    const status = screen.getByRole("status");
    expect(status).toHaveTextContent(i18n.t("changes.approvedAsProposed"));
    expect(status).not.toHaveTextContent(i18n.t("changes.accepted"));
    expect(screen.getByRole("link", { name: i18n.t("changes.open") })).toHaveAttribute(
      "href",
      "/projects/banskabystrica/approvals/chg-11aa22bb",
    );
  });

  it.each(SUPPORTED_LOCALES)("writes the notice in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container } = show();

    expect(screen.getByRole("status")).toHaveTextContent(i18n.t("changes.accepted"));
    expect(screen.getByRole("link", { name: i18n.t("changes.review") })).toBeInTheDocument();
    expectNoRawKeys(container);
  });
});
