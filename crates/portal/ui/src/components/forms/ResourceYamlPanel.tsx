import { Suspense, lazy, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Alert, Button, Icon, tabPanelProps } from "../ui";
import type { YamlEditor } from "./useYamlEditor";

// Monaco is loaded when the YAML view is opened and not before: it is the heaviest thing in
// the Portal and a dialog that never shows its manifest would otherwise pay for it.
const MonacoSourceView = lazy(() => import("../../pages/models/MonacoSourceView"));

export interface ResourceYamlPanelProps<T> {
  editor: YamlEditor<T>;
  /** The tab set the panel belongs to. */
  viewsId: string;
  /** The footer under the editor: Cancel, Check, the verdict and the submit. */
  footer: ReactNode;
}

/** Copy the manifest; the view itself is where it is edited (owner 2026-10-31: no upload, no download). */
function CopyYaml({ text }: { text: string }): JSX.Element {
  const { t } = useTranslation();
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const copy = () => {
    const writing = navigator.clipboard?.writeText(text);
    if (!writing) return setState("failed");
    void writing.then(() => setState("copied")).catch(() => setState("failed"));
  };
  return (
    <span className="flex items-center gap-2">
      {state === "failed" ? (
        <span role="alert" className="text-caption text-fg-muted">
          {t("form.yamlCopyFailed")}
        </span>
      ) : null}
      <Button size="sm" variant="secondary" icon={<Icon name={state === "copied" ? "check" : "copy"} className="size-4" />} onClick={copy}>
        {state === "copied" ? t("form.yamlCopied") : t("form.yamlCopy")}
      </Button>
    </span>
  );
}

/** The YAML view of a manifest dialog: the editor, a copy of it, and what it answered. */
export function ResourceYamlPanel<T>({ editor, viewsId, footer }: ResourceYamlPanelProps<T>): JSX.Element {
  const { t } = useTranslation();
  return (
    <div {...tabPanelProps(viewsId, "yaml")} className="flex flex-col gap-3">
      <div className="flex justify-end">
        <CopyYaml text={editor.text} />
      </div>
      <div className="overflow-hidden rounded-md border border-border">
        <Suspense fallback={<p className="p-3 text-body">{t("models.loadingEditor")}</p>}>
          <MonacoSourceView value={editor.text} onChange={editor.setText} onMount={() => undefined} height="24rem" />
        </Suspense>
      </div>
      {/* What the YAML answered, right above the button that asked it (T-1395). */}
      {editor.error ? (
        <Alert role="alert" tone="danger">
          {editor.error}
        </Alert>
      ) : null}
      {editor.issues.length > 0 ? (
        <Alert role="alert" tone="danger">
          <p>{t("form.schemaErrors")}</p>
          <ul className="mt-1 list-disc pl-5">
            {editor.issues.map((issue) => (
              <li key={issue} className="font-mono text-caption">
                {issue}
              </li>
            ))}
          </ul>
        </Alert>
      ) : null}
      {footer}
    </div>
  );
}
