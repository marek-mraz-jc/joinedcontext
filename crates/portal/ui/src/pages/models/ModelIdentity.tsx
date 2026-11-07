import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { isMap, isScalar, type Document } from "yaml";
import { Button, Icon, InlineError, Input } from "../../components/ui";

/**
 * The name line under the editor's title (T-6162, owner review v2): the model's name, which a
 * pencil renames, then its id and file name, both locked. A rename writes the LinkML `title`
 * and nothing else, so the manifest name, the file path and every reference stay as they are.
 */
export interface ModelIdentityProps {
  /** What people read: the model's English title, or the name typed for a new model. */
  name: string;
  /** The manifest name; undefined while a new model has no name to derive it from. */
  id: string | undefined;
  /** A new model: its id still follows the name, until the first save fixes it. */
  creating: boolean;
  /** The published version this edit started from. */
  version?: string;
  readOnly?: boolean;
  onRename: (name: string) => void;
}

export function ModelIdentity({ name, id, creating, version, readOnly, onRename }: ModelIdentityProps): React.JSX.Element {
  const { t } = useTranslation();
  const inputId = useId();
  const [draft, setDraft] = useState<string | undefined>(undefined);
  const [empty, setEmpty] = useState(false);
  const [renamed, setRenamed] = useState(false);
  const field = useRef<HTMLInputElement>(null);
  // The pencil the person pressed is gone: the field it opened takes the focus in its place.
  const renaming = draft !== undefined;
  useEffect(() => {
    if (renaming) field.current?.focus();
  }, [renaming]);

  const commit = () => {
    const value = (draft ?? "").trim();
    if (value === "") {
      setEmpty(true);
      field.current?.focus();
      return;
    }
    onRename(value);
    setRenamed(true);
    setDraft(undefined);
    setEmpty(false);
  };

  const locked = (
    <dl className="flex flex-wrap items-center gap-x-4 gap-y-1 text-caption text-fg-muted">
      <div className="flex items-center gap-1" title={t(creating ? "models.identity.idNew" : "models.identity.idFixed")}>
        <Icon name="lock" className="size-3.5" />
        <dt>{t("models.identity.id")}</dt>
        <dd className="font-mono text-fg">{id ?? t("models.identity.noId")}</dd>
      </div>
      {id ? (
        <div className="flex items-center gap-1" title={t("models.identity.fileFixed")}>
          <Icon name="lock" className="size-3.5" />
          <dt>{t("models.identity.file")}</dt>
          <dd className="font-mono text-fg">{id}.linkml.yaml</dd>
        </div>
      ) : null}
    </dl>
  );

  return (
    // One row: the name, Rename and the version; the id and the file under Details (T-6254).
    <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-body">
      {/* A new model is named in the new-model window; here it reads like a saved one, with Rename
          (owner 2026-11-01: no second name field on the page). */}
      {draft === undefined ? (
        <span className="flex items-center gap-1">
          {/* A model without a title says so, rather than repeating its id (T-6162 judge). */}
          {/* The page's heading: the editor names the model, not itself. */}
          <h1 className={name ? "text-display font-semibold tracking-tight text-fg" : "text-display text-fg-muted"}>
            {name || t("models.identity.unnamed")}
          </h1>
          {readOnly ? null : (
            <Button
              size="sm"
              variant="ghost"
              aria-label={t("models.identity.rename", { name: name || t("models.identity.unnamed") })}
              onClick={() => setDraft(name)}
            >
              <Icon name="pencil" className="size-4" />
              <span aria-hidden>{t("models.identity.renameShort")}</span>
            </Button>
          )}
        </span>
      ) : (
        <form
          className="flex flex-wrap items-center gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            commit();
          }}
        >
          <div className="w-64 max-w-full">
            <Input
              id={inputId}
              aria-label={t("models.identity.name")}
              aria-describedby={`${inputId}-hint`}
              aria-invalid={empty || undefined}
              className="w-64 max-w-full"
              ref={field}
              value={draft}
              onChange={(event) => {
                setDraft(event.target.value);
                setEmpty(false);
              }}
              onKeyDown={(event) => {
                if (event.key === "Escape") {
                  event.stopPropagation();
                  setDraft(undefined);
                }
              }}
            />
          </div>
          <div className="flex gap-2">
            <Button size="sm" type="submit" variant="primary">
              {t("models.identity.done")}
            </Button>
            <Button size="sm" onClick={() => setDraft(undefined)}>
              {t("app.cancel")}
            </Button>
          </div>
          {/* The field's hint under the row (UI-44, owner 2026-10-21): the row stays one line. */}
          <p id={`${inputId}-hint`} className="basis-full text-caption text-fg-muted">
            {t("models.identity.nameHint")}
          </p>
          {empty ? (
            <InlineError className="basis-full">{t("models.identity.nameEmpty")}</InlineError>
          ) : null}
        </form>
      )}
      {renamed && id ? (
        <p role="status" className="text-caption text-fg-muted">
          {t("models.identity.renamed", { id })}
        </p>
      ) : null}
      {version ? (
        <span className="font-mono text-caption text-fg-muted" title={t("models.identity.version")}>
          v{version}
        </span>
      ) : null}
      {/* A new model's id follows its name as it is typed, so it shows; a saved model's id and
          file are fixed and sit under Details, out of the title row (T-6254, owner pass 3). */}
      {creating ? (
        locked
      ) : (
        <details className="text-caption text-fg-muted open:basis-full">
          <summary className="cursor-pointer">{t("models.identity.details")}</summary>
          <div className="mt-1">{locked}</div>
        </details>
      )}
    </div>
  );
}

/** The model's English title in a LinkML document: a language map keeps its other languages. */
export function setModelTitle(document: Document, name: string): void {
  const title = document.get("title", true);
  if (isMap(title)) {
    document.setIn(["title", "en"], name);
  } else if (isScalar(title) || title === undefined) {
    document.set("title", document.createNode({ en: name }));
  }
}
