import { useState } from "react";
import type { ReactNode } from "react";
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import validator from "../src/components/forms/validator";
import type { JsonSchema } from "../src/components/forms/types";
import type { Draft, Verdict } from "../src/api/drafts";
import { forPeople } from "../src/api/client";
import { controlOf } from "../src/components/forms/resourceForm";
import { useDryRunCheck } from "../src/components/forms/useDryRunCheck";
import { useResourceDraft } from "../src/components/forms/useResourceDraft";
import { useYamlSubmitQueue } from "../src/components/forms/useYamlSubmitQueue";
import { digestOf } from "../src/api/drafts";

const drafts = vi.hoisted(() => ({
  getDraft: vi.fn(),
  putDraft: vi.fn(),
  subscribeDrafts: vi.fn(() => () => undefined),
}));
vi.mock("../src/api/drafts", async (original) => ({
  ...(await original<typeof import("../src/api/drafts")>()),
  ...drafts,
}));

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return (
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{children}</I18nextProvider>
    </QueryClientProvider>
  );
}

// T-5515: the hooks ResourceFormDialogBody was split into, each on its own.

describe("useResourceDraft", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    let version = 0;
    drafts.putDraft.mockReset().mockImplementation(async () => ({ version: ++version }) as Draft);
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("writes_the_draft_once_per_digest_change", async () => {
    type Form = { name: string; port: number };
    const { rerender } = renderHook(
      ({ form }: { form: Form }) =>
        useResourceDraft<Form>({
          open: true,
          project: "p1",
          draftKind: "Endpoint",
          formData: form,
          currentDigest: digestOf(form),
        }),
      { wrapper, initialProps: { form: { name: "air", port: 1 } } },
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(600);
    });
    expect(drafts.putDraft).toHaveBeenCalledTimes(1);
    expect(drafts.putDraft).toHaveBeenLastCalledWith("p1", "Endpoint", "air", { name: "air", port: 1 }, undefined);

    // A new object holding the same manifest is no edit: nothing is written.
    rerender({ form: { name: "air", port: 1 } });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(600);
    });
    expect(drafts.putDraft).toHaveBeenCalledTimes(1);

    // Two keystrokes inside the debounce are one write, of the last manifest, at the version the
    // first write answered with.
    rerender({ form: { name: "air", port: 2 } });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    rerender({ form: { name: "air", port: 3 } });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(600);
    });
    expect(drafts.putDraft).toHaveBeenCalledTimes(2);
    expect(drafts.putDraft).toHaveBeenLastCalledWith("p1", "Endpoint", "air", { name: "air", port: 3 }, 1);
  });
});

describe("useDryRunCheck", () => {
  const SCHEMA: JsonSchema = {
    type: "object",
    properties: {
      source: {
        type: "object",
        title: "Source",
        required: ["url"],
        properties: { url: { type: "string", title: "Source URL" } },
      },
    },
  };
  const options = {
    kind: "SyncSource",
    schema: SCHEMA,
    formView: true,
    currentDigest: "d1",
    updateVerdict: () => undefined,
    canCheck: true,
    disabled: false,
    autoCheck: false,
    readYaml: () => null,
    onYamlChecked: () => undefined,
    advancedFolded: false,
    showAdvanced: () => undefined,
    advanced: false,
  };
  let input: HTMLInputElement;
  beforeEach(() => {
    // The control rjsf renders for `source.url`.
    input = document.createElement("input");
    input.id = "root_source_url";
    document.body.append(input);
  });
  afterEach(() => {
    input.remove();
  });

  it("puts_a_finding_at_spec_source_url_on_that_fields_control", () => {
    const message = "the address does not answer";
    const verdict = {
      ok: false,
      inputDigest: "d1",
      checkedAt: new Date().toISOString(),
      findings: [{ level: "error", path: "spec.source.url", message }],
    } as Verdict;
    const { result } = renderHook(() => useDryRunCheck({ ...options, formData: { source: { url: "x" } }, verdict }), {
      wrapper,
    });
    // The form holds the spec's fields at its top level, so `spec.source.url` is `source.url`.
    expect(result.current.shownErrors).toEqual({ source: { url: { __errors: [forPeople(message)] } } });
    expect(controlOf("source.url")).toBe(input);
  });

  it("names_and_focuses_the_empty_required_field_without_asking_the_server", () => {
    const onCheck = vi.fn();
    // One object across renders, as a page's state is: the message holds while the form is the one it judged.
    const formData = { source: {} };
    const { result } = renderHook(
      () => useDryRunCheck({ ...options, formData, verdict: null, onCheck }),
      { wrapper },
    );
    act(() => result.current.runCheck());
    expect(result.current.stoppedMessage).toBe(i18n.t("form.missing", { field: "Source URL" }));
    expect(document.activeElement).toBe(input);
    expect(onCheck).not.toHaveBeenCalled();
  });
});

describe("useYamlSubmitQueue", () => {
  type Form = { rate: string };
  /** A page whose schema widens for what was typed, one render after `onChange` (T-0890). */
  function usePage(submitted: (form: Form) => void, refused: (form: Form) => void) {
    const [form, setForm] = useState<Form>({ rate: "low" });
    const rates = ["low", "high", ...(["low", "high"].includes(form.rate) ? [] : [form.rate])];
    const schema: JsonSchema = { type: "object", properties: { rate: { type: "string", enum: rates } } };
    const enqueue = useYamlSubmitQueue<Form>((typed) => {
      const { errors } = validator.validateFormData(typed, schema);
      (errors.length > 0 ? refused : submitted)(typed);
    });
    return (typed: Form) => {
      setForm(typed);
      enqueue(typed);
    };
  }

  it("submits_what_was_typed_after_the_schema_widened", () => {
    const submitted = vi.fn();
    const refused = vi.fn();
    const { result } = renderHook(() => usePage(submitted, refused));
    act(() => result.current({ rate: "custom-9" }));
    // Consumed in `enqueue` itself, the narrow schema of the render before refuses it.
    expect(refused).not.toHaveBeenCalled();
    expect(submitted).toHaveBeenCalledTimes(1);
    expect(submitted).toHaveBeenCalledWith({ rate: "custom-9" });
  });

  it("consumes_each_queued_form_once", () => {
    const submitted = vi.fn();
    const { result, rerender } = renderHook(() => usePage(submitted, vi.fn()));
    act(() => result.current({ rate: "high" }));
    rerender();
    rerender();
    expect(submitted).toHaveBeenCalledTimes(1);
  });
});
