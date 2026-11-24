/** ImportProjectDialog: the title sent, a number refused, Cancel and Close (T-7585). */
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
const { ImportProjectDialog } = await import("../src/components/ImportProjectDialog");

const PLAN = {
  repositories: [{ name: "doprava", role: "main", repository: "org/doprava", head: "8c569541234" }],
  parameters: { retries: { type: "integer", default: 3 }, mode: { type: "string", enum: ["daily", "weekly"] } },
};
const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-0000002b", namespace: "doprava" },
  status: { lane: "yellow", phase: "PendingApproval" },
};

function show() {
  const sent: { search: string; parameters: FormDataEntryValue | null; displayName: FormDataEntryValue | null }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const request = input as Request;
      const url = new URL(request.url);
      if (request.method !== "POST") return new Response("{}", { headers: { "Content-Type": "application/json" } });
      const form = await request.clone().formData();
      sent.push({ search: url.search, parameters: form.get("parameters"), displayName: form.get("displayName") });
      const dry = url.search.includes("dryRun");
      return new Response(JSON.stringify(dry ? PLAN : CHANGE), {
        status: dry ? 200 : 202,
        headers: { "Content-Type": "application/json" },
      });
    }),
  );
  const onOpenChange = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <ImportProjectDialog open onOpenChange={onOpenChange} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { sent, onOpenChange };
}

async function checked(user: ReturnType<typeof userEvent.setup>, title?: string) {
  const dialog = await screen.findByRole("dialog", { name: /Import a project/ });
  await user.upload(
    within(dialog).getByLabelText(en.projectImport.file),
    new File([new Uint8Array([80, 75, 3, 4])], "doprava.zip", { type: "application/zip" }),
  );
  await user.type(within(dialog).getByLabelText(/Name/), "doprava");
  if (title) await user.type(within(dialog).getByLabelText(en.projects.displayNameLabel), title);
  await user.click(within(dialog).getByRole("button", { name: en.projectImport.check }));
  await within(dialog).findByText("org/doprava at 8c56954");
  return dialog;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("importing a project", () => {
  it("sends the title typed, lands the import, and Close hands the dialog back", async () => {
    const user = userEvent.setup();
    const { sent, onOpenChange } = show();
    const dialog = await checked(user, "Doprava city");
    expect(sent[0].displayName).toBe("Doprava city");
    expect(within(dialog).getByRole("option", { name: "daily" })).toBeInTheDocument();
    await user.selectOptions(within(dialog).getByLabelText("mode"), "weekly");
    await user.click(within(dialog).getByRole("button", { name: en.projectImport.import }));
    expect(await within(dialog).findByText(/chg-0000002b/)).toBeInTheDocument();
    const closers = within(dialog).getAllByRole("button", { name: en.resourceDelete.close });
    await user.click(closers[closers.length - 1]);
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(sent.at(-1)?.parameters).toBe(JSON.stringify({ mode: "weekly" }));
  });

  it("refuses a number that is not one before anything is imported", async () => {
    const user = userEvent.setup();
    const { sent } = show();
    const dialog = await checked(user);
    await user.type(within(dialog).getByLabelText("retries"), "many");
    await user.click(within(dialog).getByRole("button", { name: en.projectImport.import }));
    expect(await within(dialog).findByText(/^retries: type a number/)).toBeInTheDocument();
    expect(sent).toHaveLength(1);
  });

  it("Cancel closes the dialog", async () => {
    const user = userEvent.setup();
    const { onOpenChange } = show();
    const dialog = await checked(user);
    await user.click(within(dialog).getByRole("button", { name: en.form.cancel }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
