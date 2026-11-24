/** DuplicateProjectDialog: typing, refusal, proposal and closing (T-7585). */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a href="/x">{children}</a>,
}));
const { DuplicateProjectDialog } = await import("../src/components/DuplicateProjectDialog");

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000002b", namespace: "copy" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 1, update: 0, delete: 0 } },
};

function show(answer: () => Response) {
  const sent: { method: string; path: string; body: unknown }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      sent.push({
        method: request.method,
        path: new URL(request.url).pathname,
        body: request.method === "GET" ? undefined : await request.clone().json().catch(() => undefined),
      });
      return answer();
    }),
  );
  const onOpenChange = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <DuplicateProjectDialog project="helsinki" open onOpenChange={onOpenChange} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { sent, onOpenChange };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("duplicating a project", () => {
  it("refuses the project's own name and sends nothing", async () => {
    const user = userEvent.setup();
    const { sent } = show(() => new Response("{}"));
    await user.type(await screen.findByLabelText(new RegExp(en.projects.nameLabel)), "helsinki");
    expect(await screen.findByText(en.projectDuplicate.sameName)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.projectDuplicate.propose }));
    expect(sent).toEqual([]);
  });

  it("sends the new name and title, shows the change, and Close hands the dialog back", async () => {
    const user = userEvent.setup();
    const { sent, onOpenChange } = show(
      () => new Response(JSON.stringify(CHANGE), { status: 202, headers: { "Content-Type": "application/json" } }),
    );
    await user.type(await screen.findByLabelText(new RegExp(en.projects.nameLabel)), "copy");
    await user.type(screen.getByLabelText(en.projects.displayNameLabel), "A copy");
    await user.click(screen.getByRole("button", { name: en.projectDuplicate.propose }));
    expect(await screen.findByText(/chg-0000002b/)).toBeInTheDocument();
    expect(sent.find((one) => one.method === "POST")?.body).toMatchObject({ name: "copy", displayName: "A copy" });
    const dialog = screen.getByRole("dialog");
    const closers = within(dialog).getAllByRole("button", { name: en.resourceDelete.close });
    await user.click(closers[closers.length - 1]);
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("says what the server refused and Cancel closes", async () => {
    const user = userEvent.setup();
    const { onOpenChange } = show(
      () =>
        new Response(JSON.stringify({ title: "Conflict", status: 409, detail: "copy already exists" }), {
          status: 409,
          headers: { "Content-Type": "application/problem+json" },
        }),
    );
    await user.type(await screen.findByLabelText(new RegExp(en.projects.nameLabel)), "copy");
    await user.click(screen.getByRole("button", { name: en.projectDuplicate.propose }));
    expect(await screen.findByRole("alert")).toHaveTextContent("copy already exists");
    await user.click(screen.getByRole("button", { name: en.form.cancel }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
