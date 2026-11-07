/**
 * The Monaco editor itself, loaded only when the source view is opened.
 *
 * Monaco is a few megabytes and reaches for browser APIs at import time, so it is not part of
 * the Portal's main bundle and not something every other view has to carry: this module is
 * imported lazily, and everything that can be decided without an editor lives in
 * `LinkmlSourceEditor` instead.
 */
import { useEffect, useRef, useSyncExternalStore } from "react";
import type { JSX } from "react";
import Editor from "@monaco-editor/react";
import type { OnMount } from "@monaco-editor/react";
import type { Position, editor as MonacoEditor } from "monaco-editor";
import "./monaco-setup";
import { pathAt } from "../../components/yamlManifest";
import type { LineMark } from "../../components/yamlManifest";

type Monaco = Parameters<OnMount>[1];
type CodeEditor = Parameters<OnMount>[0];

export interface MonacoSourceViewProps {
  value: string;
  onChange: (value: string) => void;
  onMount: OnMount;
  height: string;
  /** A viewer reads the text and cannot change it (T-5052). */
  readOnly?: boolean;
  /** The lines a parse error or a refusal points at, marked in the text (T-5052). */
  marks?: readonly LineMark[];
  /** What the field at a path is for, shown when the pointer rests on it (T-5052). */
  describeField?: (path: (string | number)[]) => string | null;
  /** The editor's name for a screen reader. */
  ariaLabel?: string;
}

const DARK = "(prefers-color-scheme: dark)";

/**
 * Monaco paints with its own colours, not the Portal's tokens (UI-30).
 *
 * It is a canvas: it parses hex strings itself, so `bg-surface` never reaches it and the theme
 * has to be chosen by name. `vs` was hard-coded, which left a white editor — white gutter, white
 * minimap margin, black text — in the middle of a dark page for anyone whose system asks for
 * dark. The two built-in themes are picked from the same media query `tokens.css` flips on, so
 * the editor turns with the rest of the page, including when the system changes while it is open.
 */
export function useDarkTheme(): boolean {
  return useSyncExternalStore(watchTheme, isDark, () => false);
}

/** The media query, or nothing where there is no browser to ask (a server render, a test). */
function mediaQuery(): MediaQueryList | null {
  return typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia(DARK)
    : null;
}

function isDark(): boolean {
  return mediaQuery()?.matches ?? false;
}

function watchTheme(onChange: () => void): () => void {
  const media = mediaQuery();
  media?.addEventListener("change", onChange);
  return () => media?.removeEventListener("change", onChange);
}

export default function MonacoSourceView({
  value,
  onChange,
  onMount,
  height,
  readOnly = false,
  marks = [],
  describeField,
  ariaLabel,
}: MonacoSourceViewProps): JSX.Element {
  const dark = useDarkTheme();
  const editorRef = useRef<CodeEditor | null>(null);
  const monacoRef = useRef<Monaco | null>(null);
  const describeRef = useRef(describeField);

  // The editor owns its text. A `value` that only echoes what it emitted, even one keystroke
  // behind, is not written back: the controlled prop replaced the whole text with the lagging
  // render's and dropped what was typed meanwhile (live: "# edited" saved as "#ted", T-5066).
  // Anything else, a change from another view, replaces the text.
  const emitted = useRef(new Set<string>());
  const latest = useRef(value);
  useEffect(() => {
    latest.current = value;
    const mounted = editorRef.current;
    if (!mounted) return;
    const current = mounted.getValue();
    if (value === current) {
      emitted.current.clear();
    } else if (!emitted.current.has(value)) {
      const model = mounted.getModel();
      if (readOnly) {
        // A read-only editor refuses edits; its text is only ever set from outside.
        mounted.setValue(value);
      } else if (model) {
        mounted.executeEdits("", [{ range: model.getFullModelRange(), text: value, forceMoveMarkers: true }]);
        mounted.pushUndoStop();
      }
      emitted.current.clear();
    }
  }, [value, readOnly]);

  useEffect(() => {
    describeRef.current = describeField;
    const model = editorRef.current?.getModel();
    if (model && monacoRef.current) {
      monacoRef.current.editor.setModelMarkers(model, "manifest", marksToMarkers(monacoRef.current, marks));
    }
  });

  return (
    <Editor
      height={height}
      language="yaml"
      theme={dark ? "vs-dark" : "vs"}
      defaultValue={value}
      onChange={(next) => {
        emitted.current.add(next ?? "");
        onChange(next ?? "");
      }}
      onMount={(mounted, monaco) => {
        editorRef.current = mounted;
        monacoRef.current = monaco;
        // A value that changed while Monaco loaded reached no editor yet.
        if (mounted.getValue() !== latest.current) mounted.setValue(latest.current);
        const hover = monaco.languages.registerHoverProvider("yaml", {
          provideHover: (model: MonacoEditor.ITextModel, position: Position) => {
            if (model !== mounted.getModel()) {
              return null;
            }
            const path = pathAt(model.getValue(), model.getOffsetAt(position));
            const text = path ? describeRef.current?.(path) : null;
            return text ? { contents: [{ value: `**${path?.join(".")}**` }, { value: text }] } : null;
          },
        });
        mounted.onDidDispose(() => hover.dispose());
        const model = mounted.getModel();
        if (model) {
          monaco.editor.setModelMarkers(model, "manifest", marksToMarkers(monaco, marks));
        }
        onMount(mounted, monaco);
      }}
      options={{
        minimap: { enabled: false },
        fontSize: 13,
        tabSize: 2,
        scrollBeyondLastLine: false,
        // A long line wraps under its indent: at a phone's width it was cut off with no cue.
        wordWrap: "on",
        wrappingIndent: "indent",
        renderWhitespace: "boundary",
        readOnly,
        folding: true,
        foldingStrategy: "indentation",
        ariaLabel,
      }}
    />
  );
}

function marksToMarkers(monaco: Monaco, marks: readonly LineMark[]) {
  return marks.map((mark) => ({
    severity: monaco.MarkerSeverity.Error,
    message: mark.message,
    startLineNumber: mark.line,
    endLineNumber: mark.line,
    startColumn: 1,
    endColumn: 1000,
  }));
}
