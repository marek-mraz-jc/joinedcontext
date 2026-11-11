/**
 * The JSON editor of a Dashboard manifest (T-6791), loaded only when the page opens: Monaco with
 * the JSON language service, validating and completing from the kind's schema.
 */
import { useEffect, useRef } from "react";
import type { JSX } from "react";
import Editor from "@monaco-editor/react";
import type { OnMount } from "@monaco-editor/react";
import "../models/monaco-setup";
import { jsonDefaults } from "monaco-editor/languages/features/json/register";
import { useDarkTheme } from "../models/MonacoSourceView";

type CodeEditor = Parameters<OnMount>[0];

/** The model the schema is bound to, so no other JSON editor of the Portal takes it. */
const MODEL = "inmemory://dashboard/manifest.json";
const SCHEMA_URI = "https://joinedcontext.com/schemas/kinds/Dashboard.json";

export default function DashboardJsonMonaco({
  value,
  onChange,
  schema,
  ariaLabel,
  height,
}: {
  value: string;
  onChange: (value: string) => void;
  schema: Record<string, unknown> | null;
  ariaLabel: string;
  height: string;
}): JSX.Element {
  const dark = useDarkTheme();
  const editorRef = useRef<CodeEditor | null>(null);
  // The editor owns its text: a value that only echoes what it emitted, even a keystroke behind,
  // is not written back (T-5066); a value from outside (the widget picker) replaces it.
  const emitted = useRef(new Set<string>());

  useEffect(() => {
    const mounted = editorRef.current;
    if (!mounted || mounted.getValue() === value) {
      emitted.current.clear();
      return;
    }
    if (!emitted.current.has(value)) {
      mounted.setValue(value);
    }
  }, [value]);

  useEffect(() => {
    jsonDefaults.setDiagnosticsOptions({
      validate: true,
      allowComments: false,
      schemas: schema ? [{ uri: SCHEMA_URI, fileMatch: [MODEL], schema }] : [],
    });
  }, [schema]);

  return (
    <Editor
      height={height}
      language="json"
      path={MODEL}
      theme={dark ? "vs-dark" : "vs"}
      defaultValue={value}
      onChange={(next) => {
        emitted.current.add(next ?? "");
        onChange(next ?? "");
      }}
      onMount={(mounted) => {
        editorRef.current = mounted;
        if (mounted.getValue() !== value) mounted.setValue(value);
      }}
      options={{
        ariaLabel,
        minimap: { enabled: false },
        tabSize: 2,
        scrollBeyondLastLine: false,
        wordWrap: "on",
        fixedOverflowWidgets: true,
        quickSuggestions: { strings: true, other: true, comments: false },
      }}
    />
  );
}
