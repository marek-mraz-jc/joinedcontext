/** Root dialogs and the activity feed: the controls no other test reached (T-7585). */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ to, params, children }: { to: string; params?: Record<string, string>; children: React.ReactNode }) => (
    <a href={Object.entries(params ?? {}).reduce((path, [key, value]) => path.replace(`$${key}`, value), to)}>{children}</a>
  ),
}));

const { ActivityFeed } = await import("../src/components/ActivityFeed");
const { AlertDialog } = await import("../src/components/AlertDialog");
const { DeleteProjectDialog } = await import("../src/components/DeleteProjectDialog");
const { DeleteResourceDialog } = await import("../src/components/DeleteResourceDialog");

function reply(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

function wrap(node: React.ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the activity feed filters", () => {
  it("asks the server again for the kind and the source that were picked", async () => {
    class Quiet {
      addEventListener() {}
      removeEventListener() {}
      close() {}
    }
    vi.stubGlobal("EventSource", Quiet);
    const urls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = input instanceof Request ? input.url : String(input);
        urls.push(url);
        return reply(url.includes("/activity") ? { kind: "ActivityList", items: [] } : { items: [] });
      }),
    );
    const user = userEvent.setup();
    wrap(<ActivityFeed project="helsinki" />);
    await user.selectOptions(await screen.findByLabelText(en.activity.filter.kind), "config.applied");
    await user.selectOptions(screen.getByLabelText(en.activity.filter.source), "gateway");
    await waitFor(() => expect(urls.some((u) => u.includes("kind=config.applied") && u.includes("source=gateway"))).toBe(true));
  });
});

describe("the alert dialog", () => {
  it("says why a subscription was refused and closes on Escape", async () => {
    const calls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const request = input as Request;
        calls.push(request.method);
        if (request.method === "GET") return reply({ items: [] });
        return reply({ title: "Forbidden", status: 403, detail: "no right" }, 403);
      }),
    );
    const onClose = vi.fn();
    const user = userEvent.setup();
    wrap(<AlertDialog project="helsinki" scope="pipeline" target="bikes" label="bikes" onClose={onClose} />);
    await user.click(await screen.findByRole("button", { name: en.alerts.subscribe }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(calls).toContain("PUT");
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
  });
});

describe("the delete dialogs close without sending a removal", () => {
  const CHANGE = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ChangeProposal",
    metadata: { name: "chg-1", namespace: "helsinki" },
    status: { lane: "red", phase: "PendingApproval" },
  };

  it("Cancel on a project asks the parent to close, and Close does after the proposal", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const request = input as Request;
        return request.method === "DELETE" ? reply(CHANGE, 202) : reply({ items: [], metadata: {}, spec: {} });
      }),
    );
    const onOpenChange = vi.fn();
    const user = userEvent.setup();
    wrap(<DeleteProjectDialog project="helsinki" open onOpenChange={onOpenChange} />);
    await user.click(await screen.findByRole("button", { name: en.form.cancel }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
    await user.type(screen.getByRole("textbox"), "helsinki");
    await user.click(screen.getByRole("button", { name: en.projectDelete.propose }));
    const dialog = await screen.findByRole("dialog");
    const closers = await within(dialog).findAllByRole("button", { name: en.resourceDelete.close });
    await user.click(closers[closers.length - 1]);
    expect(onOpenChange).toHaveBeenCalledTimes(2);
  });

  it("Cancel on a resource asks the parent to close, and Close does after the proposal", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => reply(CHANGE, 202)));
    const onOpenChange = vi.fn();
    const user = userEvent.setup();
    wrap(
      <DeleteResourceDialog
        target={{ project: "helsinki", kind: "Endpoint", plural: "endpoints", name: "air" }}
        open
        onOpenChange={onOpenChange}
      />,
    );
    await user.click(await screen.findByRole("button", { name: en.form.cancel }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
    await user.type(screen.getByRole("textbox"), "air");
    await user.click(screen.getByRole("button", { name: en.resourceDelete.propose }));
    const dialog = await screen.findByRole("dialog");
    const buttons = await within(dialog).findAllByRole("button", { name: en.resourceDelete.close });
    await user.click(buttons[buttons.length - 1]);
    expect(onOpenChange).toHaveBeenCalledTimes(2);
  });
});
