// covers: src/pages/spaces/SpaceInside.tsx
/** DM-113: the bulk delete asks why in its confirm step, as Explore's row delete does, and sends nothing without it. */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RichRow } from "@joinedcontext/sdk";
import i18n from "../src/i18n";
import { BulkBar } from "../src/pages/spaces/SpaceInside";

const row = (n: number) => ({ id: `urn:ngsi-ld:Event:${n}`, cells: {}, raw: { id: `urn:ngsi-ld:Event:${n}`, type: "Event" } }) as unknown as RichRow;

function show(remove: (id: string, reason: string) => Promise<void>) {
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
          onDeleted={() => undefined}
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
    expect(reason).toHaveAccessibleDescription(/for example: Duplicates left by the old feed/);
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
});
