/**
 * T-7585: the New project dialog's remaining branches: a project the organization lets anyone open
 * is entered at once, Cancel and Close wipe what was typed, and the description is sent.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { useState, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";

const navigate = vi.fn();
vi.mock("@tanstack/react-router", () => ({
  Link: ({ children }: { children: ReactNode }) => <a href="/">{children}</a>,
  useNavigate: () => navigate,
}));

const { NewProjectDialog } = await import("../src/components/layout/NewProject");

let posted: string[];
let phase: string;

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

beforeEach(async () => {
  await i18n.changeLanguage("en");
  navigate.mockReset();
  posted = [];
  phase = "Merged";
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      if (new URL(request.url).pathname.endsWith("/permissions/me")) {
        return json({ project: "p", grants: [], projects: { creation: { allowed: true } } });
      }
      posted.push(await request.clone().text());
      return json(
        {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "Change",
          metadata: { name: "chg-0000002a", namespace: "org" },
          status: { lane: "yellow", phase },
        },
        201,
      );
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

function Host({ onOpenChange }: { onOpenChange: (open: boolean) => void }): React.JSX.Element {
  const [open, setOpen] = useState(true);
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        reopen
      </button>
      <NewProjectDialog
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          onOpenChange(next);
        }}
      />
    </>
  );
}

function show(onOpenChange: (open: boolean) => void = () => {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <Host onOpenChange={onOpenChange} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("opening a project from the dialog", () => {
  it("sends the description and enters the merged project at once", async () => {
    const onOpenChange = vi.fn();
    show(onOpenChange);
    await userEvent.type(screen.getByLabelText(/^Name/), "doprava");
    await userEvent.type(screen.getByLabelText(en.projects.descriptionLabel), "Roads");
    await userEvent.click(screen.getByRole("button", { name: en.projects.open }));
    await waitFor(() => expect(navigate).toHaveBeenCalledTimes(1));
    expect(JSON.parse(posted[0]!)).toMatchObject({ name: "doprava", description: "Roads" });
    expect(navigate).toHaveBeenCalledWith({
      to: "/projects/$project/$plural",
      params: { project: "doprava", plural: "spaces" },
    });
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("forgets what was typed when Cancel is pressed", async () => {
    const onOpenChange = vi.fn();
    show(onOpenChange);
    await userEvent.type(screen.getByLabelText(/^Name/), "doprava");
    await userEvent.type(screen.getByLabelText(en.projects.displayNameLabel), "Doprava");
    await userEvent.type(screen.getByLabelText(en.projects.descriptionLabel), "Roads");
    await userEvent.click(screen.getByRole("button", { name: en.form.cancel }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(posted).toEqual([]);
    await userEvent.click(screen.getByRole("button", { name: "reopen" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByLabelText(/^Name/)).toHaveValue("");
    expect(within(dialog).getByLabelText(en.projects.displayNameLabel)).toHaveValue("");
    expect(within(dialog).getByLabelText(en.projects.descriptionLabel)).toHaveValue("");
  });

  it("shows the change awaiting approval and closes it with Close", async () => {
    phase = "PendingApproval";
    const onOpenChange = vi.fn();
    show(onOpenChange);
    await userEvent.type(screen.getByLabelText(/^Name/), "doprava");
    await userEvent.click(screen.getByRole("button", { name: en.projects.open }));
    expect(await screen.findByText(/chg-0000002a/)).toBeInTheDocument();
    expect(navigate).not.toHaveBeenCalled();
    const dialog = screen.getByRole("dialog");
    await userEvent.click(within(dialog).getAllByRole("button", { name: en.resourceDelete.close }).at(-1)!);
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
