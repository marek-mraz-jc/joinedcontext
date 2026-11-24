// T-7706: useDecision: a failure that is not an API problem still shows in words, an Error's own and the generic one for anything else.
import { renderHook, waitFor, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { useDecision } from "../src/api/decision";

vi.mock("../src/api/odata", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/api/odata")>();
  return {
    ...original,
    approveChange: vi.fn(async () => {
      throw new Error("the merge request is locked");
    }),
    rejectChange: vi.fn(async () => {
      throw "not an error object";
    }),
  };
});

const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={new QueryClient({ defaultOptions: { mutations: { retry: false } } })}>
    <I18nextProvider i18n={i18n}>{children}</I18nextProvider>
  </QueryClientProvider>
);

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("a decision that fails without an API problem", () => {
  it("says an Error's own message when approving", async () => {
    const { result } = renderHook(() => useDecision("helsinki", "chg-1"), { wrapper });
    act(() => result.current.approve.mutate(undefined));
    await waitFor(() => expect(result.current.error).toBe("the merge request is locked"));
    expect(result.current.refused).toBe(false);
  });

  it("says the generic words when rejecting fails with something that is no Error", async () => {
    const { result } = renderHook(() => useDecision("helsinki", "chg-1"), { wrapper });
    act(() => result.current.reject.mutate("no"));
    await waitFor(() => expect(result.current.error).toBe(en.app.error.generic));
  });
});
