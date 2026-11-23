// covers: src/pages/spaces/SpaceInside.tsx
/** DM-113: the bulk delete asks why in its confirm step, as Explore's row delete does, and sends nothing without it. */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RichRow } from "@joinedcontext/sdk";
import i18n from "../src/i18n";
import { BulkBar, BulkSetDialog } from "../src/pages/spaces/SpaceInside";

const row = (n: number) => ({ id: `urn:ngsi-ld:Event:${n}`, cells: {}, raw: { id: `urn:ngsi-ld:Event:${n}`, type: "Event" } }) as unknown as RichRow;

function show(remove: (id: string, reason: string) => Promise<void>, onDeleted: (kept: string[]) => void = () => undefined) {
  const source = { query: async () => ({ rows: [] }), get: async () => null, remove } as never;
  render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient()}>
        <BulkBar
          project="helsinki"
          space="events"
          source={source}
          count={2}
          rows={[row(1), row(2)]}
          canDelete
          writable={[]}
          writeSource={source}
          onChanged={() => undefined}
          exportLinks={null}
          tooManyToExport={false}
          onClear={() => undefined}
          onDeleted={onDeleted}
        />
      </QueryClientProvider>
    </I18nextProvider>,
  );
}

afterEach(() => vi.unstubAllGlobals());

describe("the bulk delete", () => {
  it("asks why, sends nothing without a reason, and deletes each row with it", async () => {
    await i18n.changeLanguage("en");
    const fetchMock = vi.fn(() => Promise.resolve(new Response(JSON.stringify({ id: 9, urn: "u", type: "Event", entity: {}, deletedAt: "", expiresAt: "" }), { status: 201, headers: { "content-type": "application/json" } })));
    vi.stubGlobal("fetch", fetchMock);
    const remove = vi.fn(() => Promise.resolve());
    show(remove);
    await userEvent.click(screen.getByRole("button", { name: "Delete selected" }));
    const dialog = await screen.findByRole("alertdialog");
    const reason = within(dialog).getByLabelText(/Reason/);
    expect(reason).toHaveAccessibleDescription(/recorded with each deletion/);
    await userEvent.click(within(dialog).getByTestId("confirm-accept"));
    expect(within(dialog).getByText("Enter why you delete them.")).toBeInTheDocument();
    expect(fetchMock).not.toHaveBeenCalled();
    expect(remove).not.toHaveBeenCalled();

    await userEvent.type(reason, "Duplicates left by the old feed");
    await userEvent.click(within(dialog).getByTestId("confirm-accept"));
    await waitFor(() => expect(remove).toHaveBeenCalledTimes(2));
    expect(remove).toHaveBeenCalledWith("urn:ngsi-ld:Event:1", "Duplicates left by the old feed");
    expect(remove).toHaveBeenCalledWith("urn:ngsi-ld:Event:2", "Duplicates left by the old feed");
  });

  it("names each row the gateway refused and keeps only those checked (T-7678)", async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(new Response(JSON.stringify({ id: 9, urn: "u", type: "Event", entity: {}, deletedAt: "", expiresAt: "" }), { status: 201, headers: { "content-type": "application/json" } }))));
    const remove = vi.fn((id: string) => (id.endsWith(":2") ? Promise.reject(new Error("the event is referenced")) : Promise.resolve()));
    const onDeleted = vi.fn();
    show(remove, onDeleted);
    await userEvent.click(screen.getByRole("button", { name: "Delete selected" }));
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.type(within(dialog).getByLabelText(/Reason/), "Cancelled events");
    await userEvent.click(within(dialog).getByTestId("confirm-accept"));
    await waitFor(() => expect(onDeleted).toHaveBeenCalledWith(["urn:ngsi-ld:Event:2"]));
    expect(screen.getByText(/urn:ngsi-ld:Event:2: .*the event is referenced/)).toBeInTheDocument();
  });

  it("calls nothing deleted when the gateway refused every row (T-7678)", async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(new Response(JSON.stringify({ id: 9, urn: "u", type: "Event", entity: {}, deletedAt: "", expiresAt: "" }), { status: 201, headers: { "content-type": "application/json" } }))));
    const onDeleted = vi.fn();
    show(() => Promise.reject("refused"), onDeleted);
    await userEvent.click(screen.getByRole("button", { name: "Delete selected" }));
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.type(within(dialog).getByLabelText(/Reason/), "Cancelled events");
    await userEvent.click(within(dialog).getByTestId("confirm-accept"));
    await waitFor(() => expect(screen.getAllByText(/: .*refused/)).toHaveLength(2));
    expect(onDeleted).not.toHaveBeenCalled();
  });
});

describe("setting a field on the checked rows (T-7678)", () => {
  it("names every row when the surface takes no writes, and opens clean the next time", async () => {
    await i18n.changeLanguage("en");
    const onDone = vi.fn();
    const onOpenChange = vi.fn();
    const slot = { name: "status", range: "string", kind: "Property" } as never;
    const view = (open: boolean) => (
      <I18nextProvider i18n={i18n}>
        <BulkSetDialog open={open} onOpenChange={onOpenChange} rows={[row(1)]} slots={[slot]} source={{}} onDone={onDone} />
      </I18nextProvider>
    );
    const { rerender } = render(view(true));
    const dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(within(dialog).getByLabelText("Field"), "status");
    await userEvent.type(within(dialog).getByLabelText("New value"), "cancelled");
    await userEvent.type(within(dialog).getByLabelText("Why"), "the organiser called it off");
    await userEvent.click(within(dialog).getByRole("button", { name: /^Set/ }));
    expect(await within(dialog).findByText(/urn:ngsi-ld:Event:1: this surface does not take writes/)).toBeInTheDocument();
    expect(onDone).not.toHaveBeenCalled();
    await userEvent.keyboard("{Escape}");
    expect(onOpenChange).toHaveBeenCalledWith(false);
    rerender(view(false));
    rerender(view(true));
    expect(within(await screen.findByRole("dialog")).queryByText(/does not take writes/)).toBeNull();
  });
});
