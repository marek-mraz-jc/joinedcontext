/**
 * The YAML view of a form dialog and the footer under it: reading a YAML that is no manifest,
 * copying the text when the browser allows and when it does not, and a verdict's age moving on
 * while the dialog stays open (T-7585).
 */
import type { ReactNode } from "react";
import { act, render, renderHook, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Verdict } from "../src/api/drafts";
import { ResourceFormFooter } from "../src/components/forms/ResourceFormFooter";
import { ResourceYamlPanel } from "../src/components/forms/ResourceYamlPanel";
import { useYamlEditor } from "../src/components/forms/useYamlEditor";
import type { ManifestSource } from "../src/components/forms/types";

vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value, onMount }: { value: string; onMount: () => void }) => {
    onMount();
    return <textarea aria-label="YAML" value={value} readOnly />;
  },
}));

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{children}</I18nextProvider>
    </QueryClientProvider>
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

const source: ManifestSource<{ name: string }> = {
  toManifest: (form) => ({ metadata: { name: form.name } }),
  fromManifest: (manifest) => ({ name: String((manifest as { metadata: { name: string } }).metadata.name) }),
};

describe("useYamlEditor", () => {
  it("reads nothing for a dialog without a source", () => {
    const { result } = renderHook(() => useYamlEditor(undefined), { wrapper });
    expect(result.current.read()).toBeNull();
    expect(result.current.error).toBeNull();
  });

  it("says a YAML that is no manifest is not one", () => {
    const { result } = renderHook(() => useYamlEditor(source), { wrapper });
    act(() => result.current.setText("just a sentence"));
    let form: unknown;
    act(() => {
      form = result.current.read();
    });
    expect(form).toBeNull();
    expect(result.current.error).toBe(`Correct the YAML: ${en.form.yamlNotManifest}`);
  });
});

describe("ResourceYamlPanel copy button", () => {
  function panel(text: string) {
    const editor = {
      text,
      setText: vi.fn(),
      error: null,
      issues: [],
      setIssues: vi.fn(),
      read: vi.fn(),
      showForm: vi.fn(),
      reset: vi.fn(),
    };
    render(<ResourceYamlPanel editor={editor} viewsId="views" footer={<span>footer</span>} />, { wrapper });
    return editor;
  }

  it("copies the text and says so", async () => {
    const writeText = vi.fn(async () => undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    panel("kind: Policy\n");
    await userEvent.click(screen.getByRole("button", { name: en.form.yamlCopy }));
    expect(writeText).toHaveBeenCalledWith("kind: Policy\n");
    expect(await screen.findByRole("button", { name: en.form.yamlCopied })).toBeInTheDocument();
  });

  it("says the browser refused when the write is rejected", async () => {
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText: vi.fn(async () => Promise.reject(new Error("denied"))) } });
    panel("kind: Policy\n");
    await userEvent.click(screen.getByRole("button", { name: en.form.yamlCopy }));
    expect(await screen.findByRole("alert")).toHaveTextContent(en.form.yamlCopyFailed);
  });

  it("says the browser refused when there is no clipboard at all", async () => {
    vi.stubGlobal("navigator", { ...navigator, clipboard: undefined });
    panel("kind: Policy\n");
    await userEvent.click(screen.getByRole("button", { name: en.form.yamlCopy }));
    expect(screen.getByRole("alert")).toHaveTextContent(en.form.yamlCopyFailed);
  });
});

describe("ResourceFormFooter verdict age", () => {
  it("moves the age on every fifteen seconds while the verdict is shown", () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    const verdict = { ok: true, inputDigest: "d1", checkedAt: new Date().toISOString(), findings: [] } as unknown as Verdict;
    const { unmount } = render(
      <ResourceFormFooter
        onCancel={vi.fn()}
        stoppedMessage={null}
        verdict={{ verdict, state: "green", refused: false }}
        lax={false}
        error={null}
      />,
      { wrapper },
    );
    expect(screen.getByTestId("draft-verdict")).toHaveTextContent("Checked 0 s ago");
    act(() => {
      vi.advanceTimersByTime(15_000);
    });
    expect(screen.getByTestId("draft-verdict")).toHaveTextContent("Checked 15 s ago");
    unmount();
  });
});
