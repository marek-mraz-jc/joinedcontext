import { useState } from "react";
import { useTranslation } from "react-i18next";
import { parse as parseYaml, stringify as stringifyYaml } from "yaml";
import type { ManifestSource } from "./types";

export interface YamlEditor<T> {
  text: string;
  setText: (text: string) => void;
  error: string | null;
  issues: string[];
  setIssues: (issues: string[]) => void;
  /** The form the YAML describes, or `null` with the reason on screen. */
  read: () => T | null;
  /** The YAML view opens on the manifest the form holds. */
  showForm: (form: T) => void;
  /** A dialog closed: what the YAML last answered goes with it. */
  reset: () => void;
}

/** The YAML view's text and what reading it answered, for a dialog with a `source`. */
export function useYamlEditor<T>(source: ManifestSource<T> | undefined): YamlEditor<T> {
  const { t } = useTranslation();
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [issues, setIssues] = useState<string[]>([]);
  const failed = (err: unknown) =>
    setError(t("form.yamlError", { message: err instanceof Error ? err.message : String(err) }));

  function read(): T | null {
    if (!source) {
      return null;
    }
    try {
      const document: unknown = parseYaml(text);
      if (!document || typeof document !== "object") {
        throw new Error(t("form.yamlNotManifest"));
      }
      setError(null);
      return source.fromManifest(document);
    } catch (err) {
      failed(err);
      return null;
    }
  }

  return {
    text,
    setText: (next) => {
      setText(next);
      setError(null);
    },
    error,
    issues,
    setIssues,
    read,
    showForm: (form) => {
      if (!source) return;
      setText(stringifyYaml(source.toManifest(form)));
      setError(null);
    },
    reset: () => {
      setError(null);
      setIssues([]);
    },
  };
}
