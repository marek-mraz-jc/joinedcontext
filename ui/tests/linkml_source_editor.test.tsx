/** T-0219: the YAML view of the same document, with the metamodel's own diagnostics (DM-13, DM-14). */
import { useEffect, useState } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { expectNoRawKeys, expectNoViolations, focusables } from "./checks";

// Monaco draws on a canvas and starts a worker, neither of which exists in jsdom. The stand-in
// is a textarea with the same contract, so what the test exercises is this component's own
// work: the value it holds, the markers it computes and the completions it offers.
const monaco = vi.hoisted(() => ({
  setModelMarkers: vi.fn(),
  registerCompletionItemProvider: vi.fn(),
  dispose: vi.fn(),
}));

const OWN_MODEL = { uri: "inmemory://model" };

function MockEditor({
  value,
  onChange,
  onMount,
  readOnly,
}: {
  value: string;
  onChange?: (value: string) => void;
  onMount?: (editor: unknown, api: unknown) => void;
  readOnly?: boolean;
}) {
    useEffect(() => {
      onMount?.(
        { getModel: () => OWN_MODEL },
        {
          editor: { setModelMarkers: monaco.setModelMarkers },
          languages: {
            registerCompletionItemProvider: monaco.registerCompletionItemProvider,
            CompletionItemKind: { Property: 9 },
          },
        },
      );
      // The stand-in mounts once, exactly as the editor it replaces does.
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
  return (
    <textarea
      aria-label="LinkML source"
      readOnly={readOnly}
      value={value}
      onChange={(event) => onChange?.(event.target.value)}
    />
  );
}

vi.mock("../src/pages/models/MonacoSourceView", () => ({ default: MockEditor }));

const { LinkmlSourceEditor, completionsFor, markersFrom, MARKER_SEVERITY } = await import(
  "../src/pages/models/LinkmlSourceEditor"
);
const { LinkmlVisualEditor } = await import("../src/pages/models/LinkmlVisualEditor");
const { diagnose, parseModel } = await import("../src/pages/models/linkml");

const SOURCE = `id: https://banskabystrica.sk/models/air
name: air
prefixes:
  bb: https://banskabystrica.sk/terms/
classes:
  AirQualityObserved:
    class_uri: bb:AirQualityObserved
    slots:
      - pm10
slots:
  pm10:
    range: float
    slot_uri: bb:pm10
    unit:
      exact_mappings: ["ucefact:GQ"]
`;

/** Both views over one string, which is what the page holds. */
function Harness({ initial = SOURCE }: { initial?: string }) {
  const [source, setSource] = useState(initial);
  const diagnostics = diagnose(source);
  return (
    <>
      <LinkmlSourceEditor source={source} onChange={setSource} diagnostics={diagnostics} />
      <LinkmlVisualEditor source={source} onChange={setSource} diagnostics={diagnostics} />
    </>
  );
}

function renderEditor(initial?: string) {
  const view = render(
    <I18nextProvider i18n={i18n}>
      <Harness initial={initial} />
    </I18nextProvider>,
  );
  return {
    unmount: view.unmount,
    area: () => screen.getByLabelText("LinkML source") as HTMLTextAreaElement,
    user: userEvent.setup(),
  };
}

describe("LinkML source editor", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    monaco.setModelMarkers.mockClear();
    monaco.registerCompletionItemProvider.mockClear();
    monaco.dispose.mockClear();
  });

  it("turns diagnostics into markers on the line they are about", () => {
    const markers = markersFrom([
      { line: 12, column: 3, severity: "error", message: "no such slot" },
      { line: 4, column: 1, severity: "warning", message: "no class_uri" },
    ]);
    expect(markers[0]).toMatchObject({
      startLineNumber: 12,
      startColumn: 3,
      message: "no such slot",
      severity: MARKER_SEVERITY.error,
    });
    expect(markers[1].severity).toBe(MARKER_SEVERITY.warning);
  });

  it("completes the metamodel, and the values a key takes where the line names one", () => {
    expect(completionsFor("  ").map((entry) => entry.label)).toContain("slot_uri");
    // T-1093: the metamodel the editor reads and the classifier judges is the one it suggests,
    // so a hierarchy key is not something a person has to know by heart.
    for (const key of ["is_a", "mixins", "subsets", "imports", "pattern", "minimum_value"]) {
      expect(completionsFor("  ").map((entry) => entry.label), key).toContain(key);
    }
    expect(completionsFor("      ngsi_ld_kind: ").map((entry) => entry.label)).toEqual([
      "Property",
      "GeoProperty",
      "Relationship",
      "LanguageProperty",
      "ListProperty",
      "JsonProperty",
      "VocabProperty",
    ]);
    expect(completionsFor("    range: fl").map((entry) => entry.label)).toContain("float");
    expect(completionsFor("      ucum_code: ").map((entry) => entry.detail)).toContain(
      "GQ · microgram per cubic metre",
    );
  });

  it("paints the model's markers into the editor and registers the completions", async () => {
    renderEditor(SOURCE.replace("    class_uri: bb:AirQualityObserved\n", ""));

    // The editor is loaded lazily, so the markers arrive on the tick after the first render.
    await waitFor(() =>
      expect(monaco.registerCompletionItemProvider).toHaveBeenCalledWith(
        "yaml",
        expect.anything(),
      ),
    );
    const markers = monaco.setModelMarkers.mock.calls.at(-1)?.[2] as { message: string }[];
    expect(markers.some((marker) => marker.message.includes("class_uri"))).toBe(true);
  });

  it("offers its completions only in its own editor, not another YAML editor of the page (F-R2UI3-014)", async () => {
    renderEditor(SOURCE);
    await waitFor(() => expect(monaco.registerCompletionItemProvider).toHaveBeenCalled());
    const provider = monaco.registerCompletionItemProvider.mock.calls.at(-1)?.[1] as {
      provideCompletionItems: (model: unknown, position: unknown) => { suggestions: unknown[] };
    };
    const model = (line: string) => ({
      getValueInRange: () => line,
      getWordUntilPosition: () => ({ startColumn: 1, endColumn: 1 }),
    });
    const position = { lineNumber: 1, column: 5 };
    expect(provider.provideCompletionItems(model("  "), position).suggestions).toEqual([]);
    expect(provider.provideCompletionItems(Object.assign(OWN_MODEL, model("  ")), position).suggestions.length).toBeGreaterThan(0);
  });

  it("withdraws its completions when it closes, so a reopened editor offers each once", async () => {
    // F-UI2-014: the provider is registered for every YAML editor of the page, not this one.
    monaco.registerCompletionItemProvider.mockImplementation(() => ({ dispose: monaco.dispose }));
    const first = renderEditor(SOURCE);
    await waitFor(() => expect(monaco.registerCompletionItemProvider).toHaveBeenCalled());
    const registered = monaco.registerCompletionItemProvider.mock.calls.length;
    first.unmount();
    expect(monaco.dispose).toHaveBeenCalledTimes(registered);
  });

  it("shows a YAML syntax error with the line it is on", async () => {
    renderEditor("classes:\n  Air:\n   slots: [pm10\n");

    expect(await screen.findByText(/line 4:/)).toBeInTheDocument();
    expect(
      await screen.findByText(/must be sufficiently indented/),
    ).toBeInTheDocument();
  });

  it("carries an edit made in the source through to the structured view", async () => {
    const { area, user } = renderEditor();

    await user.click(screen.getByRole("button", { name: "Edit source" }));
    await user.clear(area());
    await user.paste(SOURCE.replace("      - pm10", "      - pm10\n      - pm25").replace(
      "    slot_uri: bb:pm10\n",
      "    slot_uri: bb:pm10\n  pm25:\n    range: float\n    slot_uri: bb:pm25\n",
    ));

    expect(await screen.findByRole("button", { name: "pm25" })).toBeInTheDocument();
  });

  it("carries an edit made in the structured view through to the source", async () => {
    const { area, user } = renderEditor();

    await user.click(screen.getByRole("button", { name: "pm10" }));
    await user.selectOptions(screen.getByLabelText("NGSI-LD kind"), "GeoProperty");

    expect(area().value).toContain("ngsi_ld_kind: GeoProperty");
    expect(parseModel(area().value).slots[0].kind).toBe("GeoProperty");
  });

  it("opens the source read-only, and types into it only after Edit source (T-6106)", async () => {
    const { area, user } = renderEditor();

    expect(area()).toHaveAttribute("readonly");
    const edit = screen.getByRole("button", { name: "Edit source" });
    expect(edit).toHaveAttribute("aria-pressed", "false");
    await user.click(edit);
    expect(area()).not.toHaveAttribute("readonly");
    await user.click(screen.getByRole("button", { name: "Done editing" }));
    expect(area()).toHaveAttribute("readonly");
  });

  it("offers no Edit source to a person who may not propose the model", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <LinkmlSourceEditor source={SOURCE} onChange={() => {}} readOnly />
      </I18nextProvider>,
    );

    expect(screen.queryByRole("button", { name: "Edit source" })).toBeNull();
    expect(screen.getByLabelText("LinkML source")).toHaveAttribute("readonly");
  });

  it("says so when the model has no open problem", async () => {
    renderEditor(SOURCE);
    expect(await screen.findByText("The model has no open problems.")).toBeInTheDocument();
  });
});

/**
 * The UI contract of the source view (T-1771, UI-04, UI-15, UI-16, UI-44, UI-48): the editor and
 * its diagnostics are axe-clean with and without findings, the editor is reachable by keyboard,
 * and the summary and the empty state are written in all four locales.
 *
 * The Monaco stand-in above is a `<textarea>` with the same contract, so what is measured here
 * is this component's own markup: the region around the editor and the diagnostics list.
 */
describe("the LinkML source editor against the UI contract", () => {
  function renderAlone(initial: string) {
    const view = render(
      <I18nextProvider i18n={i18n}>
        <LinkmlSourceEditor source={initial} onChange={() => {}} diagnostics={diagnose(initial)} />
      </I18nextProvider>,
    );
    return { container: view.container, user: userEvent.setup() };
  }

  it("has no axe violation with a clean document", async () => {
    const { container } = renderAlone(SOURCE);
    await expectNoViolations(container);
  });

  it("has no axe violation when the document has an error and a warning", async () => {
    // A slot the class names but nothing defines, and a class with no `class_uri`: one of each
    // severity, so both colours and both list items are on the screen.
    const broken = SOURCE.replace("      - pm10", "      - pm10\n      - missing").replace(
      "    class_uri: bb:AirQualityObserved\n",
      "",
    );
    const { container } = renderAlone(broken);
    expect(diagnose(broken).length).toBeGreaterThan(0);
    await expectNoViolations(container);
  });

  it("gives the document itself to the keyboard, named as the source", async () => {
    const { container, user } = renderAlone(SOURCE);

    const area = screen.getByLabelText("LinkML source");
    await user.click(screen.getByRole("button", { name: "Edit source" }));
    expect(focusables(container)[1]).toBe(area);
    area.focus();
    await user.type(area, " ");
    expect(document.activeElement).toBe(area);
  });

  it.each(SUPPORTED_LOCALES)("counts the diagnostics in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container } = renderAlone(SOURCE);

    expect(
      screen.getByRole("heading", { name: i18n.t("models.diagnostics", { errors: 0, warnings: 0 }) }),
    ).toBeInTheDocument();
    expect(screen.getByText(i18n.t("models.noDiagnostics"))).toBeInTheDocument();
    expectNoRawKeys(container);
  });
});
