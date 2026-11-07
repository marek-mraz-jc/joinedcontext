import type { ReactNode } from "react";
import { render, renderHook, screen, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Verdict } from "../src/api/drafts";
import { ResourceFormFooter, verdictStateOf } from "../src/components/forms/ResourceFormFooter";
import { FillFromSentence, FormGuideLink, ResourceFormNotes } from "../src/components/forms/ResourceFormNotes";
import { queryKeys } from "../src/api/client";
import { ResourceYamlPanel } from "../src/components/forms/ResourceYamlPanel";
import { useYamlEditor } from "../src/components/forms/useYamlEditor";
import { useFormArrangement } from "../src/components/forms/useFormArrangement";
import type { ManifestSource } from "../src/components/forms/types";

vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value }: { value: string }) => <textarea aria-label="YAML" value={value} readOnly />,
}));

// T-5515: the parts ResourceFormDialogBody was split into, each rendered on its own.

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{children}</I18nextProvider>
    </QueryClientProvider>
  );
}

const VERDICT = { ok: true, inputDigest: "d1", checkedAt: new Date().toISOString(), findings: [] } as unknown as Verdict;

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("ResourceFormFooter", () => {
  it("says_the_verdict_and_why_propose_waits_and_checks_on_click", async () => {
    const onCheck = vi.fn();
    const onCancel = vi.fn();
    render(
      <ResourceFormFooter
        onCancel={onCancel}
        check={{ onClick: onCheck, disabled: false, disabledReason: "" }}
        stoppedMessage={null}
        verdict={{ verdict: VERDICT, state: verdictStateOf(VERDICT, "d2"), refused: false }}
        proposeReason={en.drafts.proposeReason.stale}
        lax={false}
        error="the check was refused"
      />,
      { wrapper },
    );
    expect(screen.getByTestId("draft-verdict")).toHaveTextContent(en.drafts.verdict.stale);
    expect(screen.getByTestId("propose-reason")).toHaveTextContent(en.drafts.proposeReason.stale);
    expect(screen.getByRole("alert")).toHaveTextContent("the check was refused");
    await userEvent.click(screen.getByRole("button", { name: en.form.check }));
    await userEvent.click(screen.getByRole("button", { name: en.form.cancel }));
    expect(onCheck).toHaveBeenCalledTimes(1);
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("reads_a_verdict_against_the_digest_it_judged", () => {
    expect(verdictStateOf(null, "d1")).toBe("none");
    expect(verdictStateOf(VERDICT, "d1")).toBe("green");
    expect(verdictStateOf(VERDICT, "d2")).toBe("stale");
    expect(verdictStateOf({ ...VERDICT, ok: false }, "d1")).toBe("red");
    expect(verdictStateOf({ ...VERDICT, waitsOn: ["c-1"] }, "d1")).toBe("waiting");
  });
});

describe("FillFromSentence", () => {
  it("asks the assistant to fill the open form of its kind (T-6163)", () => {
    render(<FillFromSentence kind="DataSource" />, { wrapper });
    expect(screen.getByRole("button", { name: en.form.fillFromSentence })).toBeVisible();
  });
});

describe("ResourceFormNotes", () => {
  it("lists_every_finding_and_the_refused_save", () => {
    const verdict = {
      ...VERDICT,
      ok: false,
      findings: [
        { level: "error", path: "spec.url", message: "the address does not answer" },
        { level: "warning", path: "", message: "the space keeps no history" },
      ],
    } as unknown as Verdict;
    render(
      <ResourceFormNotes
        conflict={null}
        saveRefused="the draft store is read-only"
        draftKind="DataSource"
        draft={null}
        verdict={verdict}
        restartsStream={false}
      />,
      { wrapper },
    );
    const findings = screen.getByTestId("draft-findings");
    expect(findings).toHaveTextContent("spec.url: the address does not answer");
    expect(findings).toHaveTextContent("the space keeps no history");
    expect(screen.getByRole("alert")).toHaveTextContent("the draft store is read-only");
    // T-6163: the sentence button sits on the tab row, and strict validation, the normal case, shows no chip.
    expect(screen.queryByRole("button", { name: en.form.fillFromSentence })).toBeNull();
    expect(screen.queryByText("strict validation")).toBeNull();
    // T-6105: no paragraph on the kind and no guide sentence above the fields.
    expect(screen.queryByTestId("form-about")).toBeNull();
    expect(screen.queryByTestId("form-guide")).toBeNull();
  });

  // T-6105: only a proposal that waits says so; approved on propose is the normal case.
  function notesWith(kind: string, verbs: string[]) {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const grants = [{ binding: "b", role: "r", scope: "organization", rule: { kinds: [kind], verbs } }];
    client.setQueryData(queryKeys.permissions("p1"), { bootstrap: false, project: "p1", grants });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <ResourceFormNotes
            project="p1"
            draftKind={kind}
            conflict={null}
            saveRefused={null}
            draft={null}
            verdict={null}
            restartsStream={false}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );
  }

  it("says_nothing_when_the_proposal_is_approved_at_once", () => {
    notesWith("DataSource", ["propose", "approve", "delete"]);
    expect(screen.queryByTestId("propose-standing")).toBeNull();
  });

});

describe("FormGuideLink", () => {
  it("opens_the_guide_page_in_a_new_tab_and_names_it", () => {
    render(<FormGuideLink kind="DataSource" href="https://docs.example.test/User-Guide/04-data-sources" />, { wrapper });
    const link = screen.getByRole("link", { name: /User Guide/ });
    expect(link).toHaveAttribute("href", "https://docs.example.test/User-Guide/04-data-sources");
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
    expect(link.getAttribute("aria-label")).toContain(en.kinds.DataSource.other);
  });

  it("renders_nothing_without_a_guide_or_with_an_address_that_is_not_a_place", () => {
    for (const href of [undefined, null, "javascript:alert(1)"]) {
      const { container } = render(<FormGuideLink kind="DataSource" href={href} />, { wrapper });
      expect(container).toBeEmptyDOMElement();
    }
  });
});

describe("ResourceYamlPanel", () => {
  type Form = { name: string };
  const SOURCE: ManifestSource<Form> = {
    toManifest: (form) => ({ kind: "DataSource", metadata: { name: form.name, namespace: "p1" } }),
    fromManifest: (manifest) => ({ name: (manifest as { metadata: { name: string } }).metadata.name }),
  };

  it("copies what the view holds and offers no file upload or download (owner 2026-10-31)", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    const { result } = renderHook(() => useYamlEditor<Form>(SOURCE), { wrapper });
    act(() => result.current.setText("kind: DataSource\nmetadata:\n  name: air\n"));
    render(<ResourceYamlPanel editor={result.current} viewsId="v" footer={null} />, { wrapper });
    await userEvent.click(screen.getByRole("button", { name: en.form.yamlCopy }));
    expect(writeText).toHaveBeenCalledWith("kind: DataSource\nmetadata:\n  name: air\n");
    expect(await screen.findByRole("button", { name: en.form.yamlCopied })).toBeVisible();
    expect(screen.queryByText(/upload/i)).toBeNull();
    expect(screen.queryByText(/download/i)).toBeNull();
  });
});

describe("useFormArrangement", () => {
  it("locks_the_name_of_a_resource_that_exists", () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ items: [] }), { status: 200 })));
    const { result } = renderHook(
      () =>
        useFormArrangement({
          open: true,
          kind: "NoSuchKind",
          schema: { type: "object", properties: { name: { type: "string" } } },
          uiSchema: { name: { "ui:help": "the name" } },
          lockedName: "air",
        }),
      { wrapper },
    );
    expect(result.current.uiSchema).toEqual({ name: { "ui:help": "the name", "ui:readonly": true } });
    expect(result.current.advancedFields).toBe(false);
    act(() => result.current.setAdvanced(true, false));
    expect(result.current.advanced).toBe(true);
  });
});
