/**
 * The JavaScript editor of a Script version (T-7084), loaded only when the workbench opens.
 */
import { useEffect, useRef } from "react";
import type { JSX } from "react";
import Editor from "@monaco-editor/react";
import type { OnMount } from "@monaco-editor/react";
import "../models/monaco-setup";
// The JavaScript grammar alone, highlighting only: no TypeScript service or worker.
import "monaco-editor/languages/definitions/javascript/register";
import { useDarkTheme } from "../models/MonacoSourceView";

type CodeEditor = Parameters<OnMount>[0];

export default function ScriptCodeMonaco({
  value,
  onChange,
  ariaLabel,
  describedBy,
}: {
  value: string;
  onChange: (value: string) => void;
  ariaLabel: string;
  describedBy?: string;
}): JSX.Element {
  const dark = useDarkTheme();
  const editorRef = useRef<CodeEditor | null>(null);
  const emitted = useRef(new Set<string>());

  // A value from outside (another script picked) replaces the text; an echo of a keystroke does not.
  useEffect(() => {
    const mounted = editorRef.current;
    if (!mounted || mounted.getValue() === value) {
      emitted.current.clear();
      return;
    }
    if (!emitted.current.has(value)) mounted.setValue(value);
  }, [value]);

  return (
    <Editor
      height="12rem"
      language="javascript"
      path="inmemory://script/script.js"
      theme={dark ? "vs-dark" : "vs"}
      defaultValue={value}
      onChange={(next) => {
        emitted.current.add(next ?? "");
        onChange(next ?? "");
      }}
      onMount={(mounted) => {
        editorRef.current = mounted;
        if (mounted.getValue() !== value) mounted.setValue(value);
        if (describedBy) mounted.getDomNode()?.querySelector("textarea")?.setAttribute("aria-describedby", describedBy);
      }}
      options={{
        ariaLabel,
        minimap: { enabled: false },
        fontSize: 13,
        tabSize: 2,
        scrollBeyondLastLine: false,
        wordWrap: "on",
        fixedOverflowWidgets: true,
        automaticLayout: true,
      }}
    />
  );
}
