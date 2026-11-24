/**
 * ApprovalsHistory: the filters folded behind one toggle on a phone, and the date range asked of
 * the API once it is submitted.
 */
import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { renderRoute } from "./pageHarness";
import { odataCall, odataValue } from "./odata";

const PATH = "/projects/helsinki/approvals";

function history() {
  const asked: Record<string, unknown>[] = [];
  const answer = (_path: string, request: Request) => {
    const call = odataCall(request.url);
    if (call?.name !== "jc_change_history") return undefined;
    asked.push(call.params);
    return odataValue({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ChangeList", items: [] });
  };
  return { answer, asked };
}

describe("the change history filters", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("folds behind a toggle that says whether it is open, and opens and closes on press", async () => {
    const user = userEvent.setup();
    const { answer } = history();
    await renderRoute({ path: PATH, answer });
    await user.click(await screen.findByRole("tab", { name: en.approvals.tabs.history }));
    const toggle = await screen.findByRole("button", { name: en.approvals.history.filtersToggle });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    const form = screen.getByRole("search", { name: en.approvals.history.filter, hidden: true });
    expect(form).toHaveClass("hidden");
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(form).toHaveClass("flex");
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
  });

  it("asks for the range between two dates only when the filters are applied", async () => {
    const user = userEvent.setup();
    const { answer, asked } = history();
    await renderRoute({ path: PATH, answer });
    await user.click(await screen.findByRole("tab", { name: en.approvals.tabs.history }));
    await screen.findByText(en.approvals.history.empty);
    await user.type(screen.getByLabelText(en.approvals.history.since, { selector: "input" }), "2026-10-01");
    await user.type(screen.getByLabelText(en.approvals.history.until, { selector: "input" }), "2026-10-31");
    expect(asked).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: en.approvals.history.apply }));
    await screen.findByText(en.approvals.noneMatch);
    expect(asked).toHaveLength(2);
    expect(JSON.stringify(asked[1])).toContain("2026-10-01");
    expect(JSON.stringify(asked[1])).toContain("2026-10-31");
  });
});
