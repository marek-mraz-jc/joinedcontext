/**
 * Release on the project's settings, the paths project_release.test.tsx leaves out: the registry
 * read failing and retried, Cancel and Close of the pin dialog, a number parameter typed wrong.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { ProjectRelease } from "../src/components/ProjectRelease";
import { renderPage } from "./page_contract";

const ENTRY = {
  repository: { name: "ovzdusie" },
  ref: "main",
  parameters: { days: 30 },
  declarations: { days: { type: "integer", default: 30 } },
  tags: [{ name: "v0.2.0", commit: "c0ffee2" }],
};
const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-00000031", namespace: "ovzdusie" },
  status: { lane: "red", phase: "PendingApproval", plan: { create: 0, update: 1, delete: 0 } },
};

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

function mount(registry: () => Response) {
  const seen: string[] = [];
  const puts: unknown[] = [];
  renderPage(<ProjectRelease project="ovzdusie" />, {
    path: "/projects/ovzdusie/settings/general",
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) {
        return json({ project: "org", bootstrap: false, grants: [{ role: "org-admin", binding: "jana", rule: { kinds: ["Project"], verbs: ["propose"] } }] });
      }
      if (url.pathname === "/api/v1/projects/ovzdusie/registry") {
        seen.push(request.method);
        if (request.method === "PUT") {
          puts.push(JSON.parse(await request.clone().text()));
          return json(CHANGE, 202);
        }
        return registry();
      }
      return undefined;
    },
  });
  return { seen, puts };
}

describe("Release, further paths", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("says the read failed and asks the registry again on Retry", async () => {
    let calls = 0;
    const { seen } = mount(() => (++calls === 1 ? json({ title: "Server Error", status: 500, detail: "registry down" }, 500) : json(ENTRY)));
    await userEvent.click(await screen.findByRole("button", { name: "Retry" }));
    expect(await screen.findByText("main")).toBeInTheDocument();
    expect(seen.filter((m) => m === "GET")).toHaveLength(2);
  });

  it("refuses a number parameter typed as text before anything is sent, and Cancel closes the dialog", async () => {
    const { puts } = mount(() => json(ENTRY));
    const user = userEvent.setup();
    const button = await screen.findByRole("button", { name: "Pin a release" });
    await waitFor(() => expect(button).toBeEnabled());
    await user.click(button);
    let dialog = await screen.findByRole("dialog", { name: /Pin a release of ovzdusie/ });
    const days = within(dialog).getByLabelText("days");
    await user.clear(days);
    await user.type(days, "soon");
    await user.click(within(dialog).getByRole("button", { name: "Propose" }));
    expect((await within(dialog).findAllByText(/days: type a number/)).length).toBeGreaterThan(0);
    expect(puts).toEqual([]);

    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // reopened: the typed text and the refusal are gone
    await user.click(screen.getByRole("button", { name: "Pin a release" }));
    dialog = await screen.findByRole("dialog", { name: /Pin a release of ovzdusie/ });
    expect(within(dialog).getByLabelText("days")).toHaveValue("30");
    expect(within(dialog).queryByText(/type a number/)).toBeNull();
  });

  it("proposes, shows the change and closes on Close", async () => {
    const { puts } = mount(() => json(ENTRY));
    const user = userEvent.setup();
    const button = await screen.findByRole("button", { name: "Pin a release" });
    await waitFor(() => expect(button).toBeEnabled());
    await user.click(button);
    const dialog = await screen.findByRole("dialog", { name: /Pin a release of ovzdusie/ });
    await user.click(within(dialog).getByRole("button", { name: "Propose" }));
    expect(await within(dialog).findByText(/chg-00000031/)).toBeInTheDocument();
    expect(puts).toEqual([{ ref: "main", parameters: { days: 30 } }]);
    await user.click(within(dialog).getAllByRole("button", { name: "Close" }).at(-1) as HTMLElement);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});
