/**
 * T-7585: the draft a form edits when the store answers badly or another window writes: a draft
 * that cannot be read, a save another window outran (409), and the events of the project's draft
 * stream (dropped, a new verdict, a newer version).
 */
import type { ReactNode } from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Draft, DraftEvent, Verdict } from "../src/api/drafts";
import { digestOf } from "../src/api/drafts";
import { useResourceDraft } from "../src/components/forms/useResourceDraft";

const drafts = vi.hoisted(() => ({
  getDraft: vi.fn(),
  putDraft: vi.fn(),
  subscribeDrafts: vi.fn(),
}));
vi.mock("../src/api/drafts", async (original) => ({
  ...(await original<typeof import("../src/api/drafts")>()),
  ...drafts,
}));

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{children}</I18nextProvider>
    </QueryClientProvider>
  );
}

type Form = { name: string; port: number };

const GREEN = { ok: true, inputDigest: "d1", checkedAt: "2026-11-01T10:00:00Z", findings: [] } as unknown as Verdict;

function draftOf(version: number, form: Form, verdict?: Verdict | null): Draft {
  return { version, manifest: form, ...(verdict !== undefined ? { verdict } : {}) } as unknown as Draft;
}

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

afterEach(() => {
  vi.useRealTimers();
});

function named(form: Form = { name: "air", port: 1 }) {
  return renderHook(
    ({ data }: { data: Form }) =>
      useResourceDraft<Form>({
        open: true,
        project: "p1",
        draftKind: "Endpoint",
        draftName: "air",
        formData: data,
        currentDigest: digestOf(data),
      }),
    { wrapper, initialProps: { data: form } },
  );
}

describe("a draft that cannot be read", () => {
  it("says why and writes nothing over it", async () => {
    drafts.getDraft.mockRejectedValue(Object.assign(new Error("boom"), { detail: "the store is down" }));
    const { result } = named();
    await waitFor(() => expect(result.current.saveRefused).toBe("the store is down"));
    expect(drafts.putDraft).not.toHaveBeenCalled();
  });
});

describe("a save another window outran", () => {
  async function conflicting(readBack: () => Promise<Draft | null>) {
    vi.useFakeTimers();
    drafts.getDraft.mockResolvedValueOnce(draftOf(1, { name: "air", port: 1 }));
    drafts.putDraft.mockRejectedValue(Object.assign(new Error("stale"), { status: 409 }));
    const hook = named();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
    drafts.getDraft.mockImplementation(readBack);
    hook.rerender({ data: { name: "air", port: 2 } });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(700);
    });
    return hook;
  }

  it("says there is a conflict and takes the other window's version", async () => {
    const { result } = await conflicting(async () => draftOf(5, { name: "air", port: 9 }, GREEN));
    expect(result.current.conflict).toBe(en.drafts.conflict);
    expect(result.current.currentDraft?.version).toBe(5);
    expect(result.current.verdict).toEqual(GREEN);
  });

  it("keeps the conflict said when the read-back fails", async () => {
    const { result } = await conflicting(async () => Promise.reject(new Error("offline")));
    expect(result.current.conflict).toBe(en.drafts.conflict);
    expect(result.current.currentDraft?.version).toBe(1);
  });
});

describe("the project's draft events", () => {
  async function open() {
    drafts.getDraft.mockResolvedValueOnce(draftOf(1, { name: "air", port: 1 }, GREEN));
    const hook = named();
    await waitFor(() => expect(hook.result.current.currentDraft?.version).toBe(1));
    await waitFor(() => expect(handler).toBeDefined());
    return hook;
  }

  it("ignores an event for another draft", async () => {
    const { result } = await open();
    act(() => handler?.({ event: "drop", kind: "Endpoint", name: "other", version: 2 } as DraftEvent));
    expect(result.current.currentDraft?.version).toBe(1);
  });

  it("forgets the draft and its verdict when it is dropped", async () => {
    const { result } = await open();
    expect(result.current.verdict).toEqual(GREEN);
    act(() => handler?.({ event: "drop", kind: "Endpoint", name: "air", version: 2 } as DraftEvent));
    expect(result.current.currentDraft).toBeNull();
    expect(result.current.verdict).toBeNull();
  });

  it("reads the draft again for a new verdict and adopts a newer one", async () => {
    const { result } = await open();
    const newer = { ...GREEN, checkedAt: "2026-11-01T11:00:00Z", inputDigest: "d2" } as Verdict;
    drafts.getDraft.mockResolvedValueOnce(draftOf(2, { name: "air", port: 1 }, newer));
    act(() => handler?.({ event: "verdict", kind: "Endpoint", name: "air", version: 2 } as DraftEvent));
    await waitFor(() => expect(result.current.verdict).toEqual(newer));
    expect(result.current.currentDraft?.version).toBe(2);
  });

  it("keeps what the form shows when the refresh for a verdict fails or finds no draft", async () => {
    const { result } = await open();
    drafts.getDraft.mockRejectedValueOnce(new Error("offline"));
    act(() => handler?.({ event: "verdict", kind: "Endpoint", name: "air", version: 2 } as DraftEvent));
    drafts.getDraft.mockResolvedValueOnce(null);
    act(() => handler?.({ event: "verdict", kind: "Endpoint", name: "air", version: 3 } as DraftEvent));
    await waitFor(() => expect(drafts.getDraft).toHaveBeenCalledTimes(3));
    expect(result.current.currentDraft?.version).toBe(1);
    expect(result.current.verdict).toEqual(GREEN);
  });

  it("reads the draft again when another window wrote a newer version, and keeps the form when that read fails", async () => {
    const { result } = await open();
    drafts.getDraft.mockResolvedValueOnce(draftOf(3, { name: "air", port: 7 }));
    act(() => handler?.({ event: "put", kind: "Endpoint", name: "air", version: 3 } as DraftEvent));
    await waitFor(() => expect(result.current.currentDraft?.version).toBe(3));
    drafts.getDraft.mockRejectedValueOnce(new Error("offline"));
    act(() => handler?.({ event: "put", kind: "Endpoint", name: "air", version: 4 } as DraftEvent));
    await waitFor(() => expect(drafts.getDraft).toHaveBeenCalledTimes(3));
    expect(result.current.currentDraft?.version).toBe(3);
  });

  it("does not read the draft again for a version it already holds", async () => {
    await open();
    act(() => handler?.({ event: "put", kind: "Endpoint", name: "air", version: 1 } as DraftEvent));
    expect(drafts.getDraft).toHaveBeenCalledTimes(1);
  });
});
