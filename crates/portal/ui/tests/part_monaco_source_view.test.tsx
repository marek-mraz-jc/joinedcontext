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
import { renderToString } from "react-dom/server";
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

  type Given = { onMount: (e: unknown, m: unknown) => void; onChange: (v: string | undefined) => void };

  /** A Monaco that records what the view registers: the hover provider, the markers, the dispose hook. */
  function recording() {
    const registered: { provideHover?: (model: unknown, position: unknown) => unknown } = {};
    const markers: unknown[][] = [];
    let disposed = 0;
    let onDispose: () => void = () => undefined;
    const fake = {
      languages: {
        registerHoverProvider: (_: string, provider: typeof registered) => {
          Object.assign(registered, provider);
          return { dispose: () => (disposed += 1) };
        },
      },
      editor: { setModelMarkers: (_: unknown, __: string, list: unknown[]) => markers.push(list) },
      MarkerSeverity: { Error: 8 },
    };
    return { fake, registered, markers, disposed: () => disposed, onDispose: () => onDispose(), setDispose: (listen: () => void) => (onDispose = listen) };
  }

  it("describes the field under the pointer, says nothing elsewhere, and lets go of the hover on dispose", () => {
    stubTheme(false);
    props.length = 0;
    const describeField = vi.fn((path: (string | number)[]) => (path.join(".") === "spec.slug" ? "The address segment." : null));
    const marks = [{ line: 2, message: "slug is required" }];
    render(
      <MonacoSourceView value={"spec:\n  slug: a\n  other: b\n"} onChange={() => undefined} onMount={() => undefined} height="24rem" describeField={describeField} marks={marks} />,
    );
    const given = props.at(-1) as unknown as Given;
    const text = "spec:\n  slug: a\n  other: b\n";
    const own = { getValue: () => text, getOffsetAt: (at: { offset: number }) => at.offset, getFullModelRange: () => ({}) };
    const editor = { ...fakeEditor(text), getModel: () => own, onDidDispose: (listen: () => void) => recordingNow.setDispose(listen) };
    const recordingNow = recording();
    act(() => given.onMount(editor, recordingNow.fake));
    expect(recordingNow.markers.at(-1)).toEqual([{ severity: 8, message: "slug is required", startLineNumber: 2, endLineNumber: 2, startColumn: 1, endColumn: 1000 }]);

    const hover = recordingNow.registered.provideHover as (model: unknown, position: unknown) => unknown;
    expect(hover(own, { offset: text.indexOf("slug") + 1 })).toEqual({ contents: [{ value: "**spec.slug**" }, { value: "The address segment." }] });
    expect(hover(own, { offset: text.indexOf("other") + 1 })).toBeNull();
    expect(hover({ getValue: () => "", getOffsetAt: () => 0 }, { offset: 0 })).toBeNull();
    expect(recordingNow.disposed()).toBe(0);
    recordingNow.onDispose();
    expect(recordingNow.disposed()).toBe(1);
    vi.unstubAllGlobals();
  });

  it("writes into the editor the text that changed while it loaded, and passes on an emptied change as the empty text", () => {
    stubTheme(false);
    props.length = 0;
    const changes: string[] = [];
    const { rerender } = render(<MonacoSourceView value="first" onChange={(next) => changes.push(next)} onMount={() => undefined} height="24rem" />);
    rerender(<MonacoSourceView value="second" onChange={(next) => changes.push(next)} onMount={() => undefined} height="24rem" />);
    const given = props.at(-1) as unknown as Given;
    const editor = fakeEditor("first");
    act(() => given.onMount(editor, recording().fake));
    expect(editor.text).toBe("second");
    act(() => given.onChange(undefined));
    expect(changes).toEqual([""]);
    vi.unstubAllGlobals();
  });

  it("replaces the whole text of a read-only editor when the value changes from outside", () => {
    stubTheme(false);
    props.length = 0;
    const view = (value: string) => <MonacoSourceView value={value} readOnly onChange={() => undefined} onMount={() => undefined} height="24rem" />;
    const { rerender } = render(view("a: 1"));
    const given = props.at(-1) as unknown as Given;
    const editor = fakeEditor("a: 1");
    act(() => given.onMount(editor, recording().fake));
    rerender(view("a: 2"));
    expect(editor.text).toBe("a: 2");
    expect(editor.edits).toEqual([]);
    vi.unstubAllGlobals();
  });

  it("is light where it is rendered on a server, which cannot ask the browser", () => {
    stubTheme(true);
    const html = renderToString(<MonacoSourceView value="name: fleet" onChange={() => undefined} onMount={() => undefined} height="24rem" />);
    expect(html).toContain('data-theme="vs"');
    vi.unstubAllGlobals();
  });
});
