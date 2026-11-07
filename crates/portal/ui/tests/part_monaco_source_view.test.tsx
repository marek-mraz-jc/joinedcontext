/**
 * T-1850: the Monaco view follows the Portal's theme (UI-01, UI-30).
 *
 * Monaco is a canvas with its own colours: it parses the theme it is given by name and never
 * reads `tokens.css`, which is why `theme` was simply missing and the editor stayed white — white
 * gutter, white margin, black text — inside a dark page. The editor itself cannot run in jsdom
 * (it wants a real canvas and a worker), so what is held here is the decision, on the props the
 * component hands it.
 */
import { render, screen, act } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const props: Record<string, unknown>[] = [];

vi.mock("@monaco-editor/react", () => ({
  default: (given: Record<string, unknown>) => {
    props.push(given);
    return <div data-testid="monaco" data-theme={String(given.theme)} />;
  },
}));

// The loader configuration reaches for a web worker on import, which jsdom has none of.
vi.mock("../src/pages/models/monaco-setup", () => ({}));

const { default: MonacoSourceView } = await import("../src/pages/models/MonacoSourceView");

interface FakeMedia {
  matches: boolean;
  listeners: ((event: MediaQueryListEvent) => void)[];
}

function stubTheme(dark: boolean): FakeMedia {
  const media: FakeMedia = { matches: dark, listeners: [] };
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: query.includes("dark") ? media.matches : false,
      media: query,
      addEventListener: (_: string, listen: (event: MediaQueryListEvent) => void) =>
        media.listeners.push(listen),
      removeEventListener: (_: string, listen: (event: MediaQueryListEvent) => void) => {
        media.listeners = media.listeners.filter((one) => one !== listen);
      },
    })),
  );
  return media;
}

function show() {
  return render(
    <MonacoSourceView value="name: fleet" onChange={() => undefined} onMount={() => undefined} height="24rem" />,
  );
}

describe("the Monaco source view", () => {
  it("opens in the dark theme when the system asks for dark", () => {
    stubTheme(true);
    show();
    expect(screen.getByTestId("monaco")).toHaveAttribute("data-theme", "vs-dark");
    vi.unstubAllGlobals();
  });

  it("opens in the light theme otherwise", () => {
    stubTheme(false);
    show();
    expect(screen.getByTestId("monaco")).toHaveAttribute("data-theme", "vs");
    vi.unstubAllGlobals();
  });

  it("turns with the system while it is open, and lets go of the query when it closes", () => {
    const media = stubTheme(false);
    const view = show();
    expect(media.listeners.length).toBeGreaterThan(0);

    act(() => {
      media.matches = true;
      for (const listen of [...media.listeners]) {
        listen({ matches: true } as MediaQueryListEvent);
      }
    });
    expect(screen.getByTestId("monaco")).toHaveAttribute("data-theme", "vs-dark");

    view.unmount();
    expect(media.listeners).toHaveLength(0);
    vi.unstubAllGlobals();
  });

  it("keeps its editor options where a browser cannot be asked at all", () => {
    // A render with no `matchMedia` — a server render, an older test environment — must not
    // throw; light is the answer, and the rest of the editor is unaffected.
    vi.stubGlobal("matchMedia", undefined);
    props.length = 0;
    show();
    expect(screen.getByTestId("monaco")).toHaveAttribute("data-theme", "vs");
    const given = props[props.length - 1];
    expect(given.language).toBe("yaml");
    expect(given.height).toBe("24rem");
    expect((given.options as { minimap: { enabled: boolean } }).minimap.enabled).toBe(false);
    vi.unstubAllGlobals();
  });
});

describe("the Monaco source view while a person types (T-5066)", () => {
  function fakeEditor(text: string) {
    const editor = {
      text,
      edits: [] as string[],
      getValue: () => editor.text,
      setValue: (next: string) => {
        editor.text = next;
      },
      getModel: () => ({ getFullModelRange: () => ({}) }),
      executeEdits: (_: string, edits: { text: string }[]) => {
        editor.text = edits[0].text;
        editor.edits.push(edits[0].text);
        return true;
      },
      pushUndoStop: () => true,
      onDidDispose: () => undefined,
    };
    return editor;
  }
  const monaco = { languages: { registerHoverProvider: () => ({ dispose: () => undefined }) }, editor: { setModelMarkers: () => undefined }, MarkerSeverity: {} };

  it("keeps what was typed when a render one keystroke behind arrives, and takes a change from elsewhere", () => {
    stubTheme(false);
    props.length = 0;
    const view = (value: string) => (
      <MonacoSourceView value={value} onChange={() => undefined} onMount={() => undefined} height="24rem" />
    );
    const { rerender } = render(view("a"));
    const given = props.at(-1) as { value?: string; onMount: (e: unknown, m: unknown) => void; onChange: (v: string) => void };
    // The editor owns its text: a controlled `value` let the library write the lagging one back.
    expect(given.value).toBeUndefined();
    const editor = fakeEditor("a");
    act(() => given.onMount(editor, monaco));

    editor.text = "ab";
    act(() => given.onChange("ab"));
    editor.text = "abc";
    act(() => given.onChange("abc"));
    rerender(view("ab"));
    expect(editor.text).toBe("abc");
    expect(editor.edits).toEqual([]);

    rerender(view("abc"));
    rerender(view("name: other"));
    expect(editor.text).toBe("name: other");
    expect(editor.edits).toEqual(["name: other"]);
    vi.unstubAllGlobals();
  });
});
