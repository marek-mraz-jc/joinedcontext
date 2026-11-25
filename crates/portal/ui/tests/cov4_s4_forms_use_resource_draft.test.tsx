// T-7706: useResourceDraft: a newer version announced by another window whose draft is gone by the read-back keeps what the form holds.
import type { ReactNode } from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import type { Draft, DraftEvent } from "../src/api/drafts";
import { digestOf } from "../src/api/drafts";
import { useResourceDraft } from "../src/components/forms/useResourceDraft";

const drafts = vi.hoisted(() => ({ getDraft: vi.fn(), putDraft: vi.fn(), subscribeDrafts: vi.fn() }));
vi.mock("../src/api/drafts", async (original) => ({
  ...(await original<typeof import("../src/api/drafts")>()),
  ...drafts,
}));

function wrapper({ children }: { children: ReactNode }) {
  return (
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>{children}</I18nextProvider>
    </QueryClientProvider>
  );
}

type Form = { name: string; port: number };
let handler: ((event: DraftEvent) => void) | undefined;

beforeEach(async () => {
  await i18n.changeLanguage("en");
  handler = undefined;
  drafts.getDraft.mockReset();
  drafts.putDraft.mockReset();
  drafts.subscribeDrafts.mockReset().mockImplementation((_project: string, onEvent: (event: DraftEvent) => void) => {
    handler = onEvent;
    return () => undefined;
  });
});

describe("a newer draft that is gone when it is read back (T-7706)", () => {
  it("keeps the draft and the form it already holds", async () => {
    const form: Form = { name: "air", port: 1 };
    drafts.getDraft.mockResolvedValueOnce({ version: 1, manifest: form } as unknown as Draft);
    const { result } = renderHook(
      () => useResourceDraft<Form>({ open: true, project: "p1", draftKind: "Endpoint", draftName: "air", formData: form, currentDigest: digestOf(form) }),
      { wrapper },
    );
    await waitFor(() => expect(result.current.currentDraft?.version).toBe(1));
    await waitFor(() => expect(handler).toBeDefined());
    drafts.getDraft.mockResolvedValueOnce(null);
    act(() => handler?.({ event: "put", kind: "Endpoint", name: "air", version: 3 } as DraftEvent));
    await waitFor(() => expect(drafts.getDraft).toHaveBeenCalledTimes(2));
    expect(result.current.currentDraft?.version).toBe(1);
  });
});
