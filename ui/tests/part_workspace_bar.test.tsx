/**
 * T-1822: the bar that says you are inside a copy, against the UI contract
 * (UI-15, UI-16, UI-61).
 *
 * The bar is the one thing on the page that says a write will not reach the project, so its
 * states are the contract: the copy, a copy that is gone, one that expired, one that is somebody
 * else's, and a read that was refused. Each is one line and a way out, in the reader's own
 * language and with its date written the way that language writes a date.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { WorkspaceBar } from "../src/components/layout/WorkspaceBar";
import { setActiveWorkspace } from "../src/components/layout/WorkspaceContext";
import { expectNoRawKeys, expectNoViolations, expectTabOrder } from "./checks";

const navigate = vi.fn();
let search: Record<string, string> = {};
vi.mock("@tanstack/react-router", () => ({
  Link: ({ children, to, params }: { children: ReactNode; to: string; params?: Record<string, string> }) => (
    <a href={Object.entries(params ?? {}).reduce((path, [k, v]) => path.replace(`$${k}`, v), to)}>{children}</a>
  ),
  useNavigate: () => navigate,
  useRouterState: ({ select }: { select: (s: { location: { search: unknown } }) => unknown }) =>
    select({ location: { search } }),
}));

let me: { email: string; username: string } = { email: "jana@hel.fi", username: "jana" };
vi.mock("../src/auth/AuthProvider", () => ({ useAuthStatus: () => "authenticated",
  useAuth: () => ({ identity: me, status: "authenticated" }),
}));

const WORKSPACE = {
  name: "air-v2",
  title: "Air cleanup",
  project: "helsinki",
  owner: "jana@hel.fi",
  branch: "workspace/air-v2",
  baseRevision: "base1",
  scope: { kind: "project" },
  previewState: "none",
  createdAt: "2026-09-18T09:00:00Z",
  // A week from the run: a fixed date expires the copy the day it passes (T-2729).
  expiresAt: new Date(Date.now() + 7 * 24 * 3600 * 1000).toISOString(),
  changes: 3,
};

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

let answer: Response;

async function bar(overrides: Partial<typeof WORKSPACE> = {}, response?: Response) {
  answer = response ?? json({ ...WORKSPACE, ...overrides });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const { WorkspaceProvider } = await import("../src/components/layout/WorkspaceContext");
  const view = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <WorkspaceProvider>
          <WorkspaceBar project="helsinki" />
        </WorkspaceProvider>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  // The steady bar is a landmark and each notice is a `status`, so both are found by the label
  // they share rather than by one role (T-1254).
  // The bar is busy from the first frame while the copy's record loads (T-1489), so wait for it
  // to settle before reading the state under test.
  await waitFor(() => expect(screen.queryByTestId("workspace-bar-loading")).toBeNull());
  const region = await screen.findByLabelText(i18n.t("workspaces.bar.label"));
  return { ...view, region, user: userEvent.setup() };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  navigate.mockReset();
  search = { workspace: "air-v2" };
  me = { email: "jana@hel.fi", username: "jana" };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const href = input instanceof Request ? input.url : String(input);
      return new URL(href, "http://localhost").pathname.endsWith("/workspaces/air-v2")
        ? answer.clone()
        : json({ title: "Not Found", status: 404 }, 404);
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  setActiveWorkspace(null);
});

describe("the copy bar against the UI contract", () => {
  it("has no axe violation, inside a copy and on each of its notices", async () => {
    const mine = await bar();
    await expectNoViolations(mine.container);
    mine.unmount();

    const gone = await bar({}, json({ title: "Not Found", status: 404 }, 404));
    await expectNoViolations(gone.container);
  });

  it("is reached in DOM order, every control a real one", async () => {
    const { region, user } = await bar();

    await expectTabOrder(user, region);
    // Everything that acts is a link or a button; nothing is a clickable span.
    for (const control of Array.from(region.querySelectorAll("a, button"))) {
      expect(control).toHaveAccessibleName();
    }
    expect(region.querySelectorAll("[onclick], div[tabindex]")).toHaveLength(0);
  });

  it("writes its date in the language the Portal is read in, not the browser's", async () => {
    const en = await bar();
    expect(en.region.textContent).toContain(new Date(WORKSPACE.createdAt).toLocaleDateString("en"));
    en.unmount();

    await i18n.changeLanguage("de");
    const de = await bar();
    const german = new Date(WORKSPACE.createdAt).toLocaleDateString("de");
    expect(de.region.textContent).toContain(german);
    expect(german).not.toBe(new Date(WORKSPACE.createdAt).toLocaleDateString("en"));
  });

  it("says a date the API sent wrong as it arrived, rather than Invalid Date", async () => {
    const { region } = await bar({ createdAt: "not-a-day" });
    expect(region.textContent).toContain("not-a-day");
    expect(region.textContent).not.toContain("Invalid Date");
  });

  it("counts 0, 1 and many changes in its own plural", async () => {
    const none = await bar({ changes: 0 });
    expect(screen.getByTestId("workspace-changes").textContent).toBe("No changes yet");
    // Nothing to bring back, so the action that would do it is not offered at all.
    expect(within(none.region).queryByText(i18n.t("workspaces.bar.bringBack"))).toBeNull();
    none.unmount();

    const one = await bar({ changes: 1 });
    expect(screen.getByTestId("workspace-changes").textContent).toBe("1 change");
    one.unmount();

    await bar({ changes: 12 });
    expect(screen.getByTestId("workspace-changes").textContent).toBe("12 changes");
  });

  it("says a copy of somebody else is read only, in words and not by colour alone (UI-30)", async () => {
    const { region } = await bar({ owner: "matej@hel.fi" });

    const chip = screen.getByTestId("workspace-foreign");
    expect(chip.textContent).toContain("matej@hel.fi");
    // The shared Badge: one border, one radius, one type step for every chip in the Portal.
    expect(chip.className).toContain("rounded-full");
    expect(chip.className).toContain("text-caption");
    expect(within(region).queryByText(i18n.t("workspaces.bar.bringBack"))).toBeNull();
  });

  it("announces each notice and leaves the bar itself a landmark", async () => {
    // A copy can expire, or be discarded by its owner, while the person is reading a page of it:
    // the line that replaces the bar has to be said, not only shown (T-1254, UI-15). The bar that
    // is simply there stays a landmark — a live region around the links and the counter would
    // read the whole bar out again on every change, and read it without its links.
    const inside = await bar();
    expect(inside.region).toHaveAttribute("role", "region");
    expect(within(inside.region).getAllByRole("link").length).toBeGreaterThan(0);
    inside.unmount();

    for (const answer of [
      json({ title: "Not Found", status: 404 }, 404),
      json({ title: "Forbidden", status: 403, detail: "not a member" }, 403),
    ]) {
      const notice = await bar({}, answer);
      expect(notice.region).toHaveAttribute("role", "status");
      expect(within(notice.region).queryByRole("link")).toBeNull();
      notice.unmount();
    }
  });

  it("explains a refusal and an expiry, and each keeps the way out", async () => {
    const refused = await bar({}, json({ title: "Forbidden", status: 403, detail: "not a member" }, 403));
    expect(refused.region.textContent).toContain("You may not open this copy");
    expect(within(refused.region).getByRole("button", { name: i18n.t("workspaces.bar.leave") })).toBeEnabled();
    refused.unmount();

    const expired = await bar({ expiresAt: "2026-01-02T09:00:00Z" });
    expect(expired.region.textContent).toContain("expired on");
    expect(within(expired.region).getByRole("button", { name: i18n.t("workspaces.bar.leave") })).toBeEnabled();
  });

  it("leaves the copy by taking the workspace out of the URL", async () => {
    const { region, user } = await bar();

    await user.click(within(region).getByRole("button", { name: i18n.t("workspaces.bar.leave") }));
    expect(navigate).toHaveBeenCalledTimes(1);
    const { search: next } = navigate.mock.calls[0][0] as { search: (prev: object) => object };
    expect(next({ workspace: "air-v2", page: "2" })).toEqual({ page: "2" });
  });

  it("holds a long title without a line it cannot break", async () => {
    const { region } = await bar({ title: "Air cleanup for the whole valley, autumn 2026, third attempt" });

    expect(region.className).toContain("flex-wrap");
    expect(region.querySelectorAll(".whitespace-nowrap, .overflow-hidden, .truncate")).toHaveLength(0);
  });

  it.each(SUPPORTED_LOCALES)("writes its own words in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container } = await bar({ owner: "matej@hel.fi" });

    expectNoRawKeys(container);
  });
});
