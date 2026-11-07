import { useState } from "react";
import { Card } from "@joinedcontext/sdk";
import { t } from "../i18n";
import type { Lang, Word } from "../i18n";
import { problemText } from "../problems";
import type { Resolutions } from "../resolutions";

export type Kind = "text" | "number" | "boolean";

/** The value as the server takes it, from the text typed and its kind; `null` when it is none. */
export function valueOf(kind: Kind, text: string): string | number | boolean | null {
  const trimmed = text.trim();
  if (kind === "boolean") return trimmed === "true" ? true : trimmed === "false" ? false : null;
  if (kind === "number") {
    const n = Number(trimmed.replace(",", "."));
    return trimmed !== "" && Number.isFinite(n) ? n : null;
  }
  return trimmed !== "" && trimmed.length <= 500 ? trimmed : null;
}

interface Props {
  lang: Lang;
  resolutions: Resolutions;
  finding: { entity: string; attribute: string; id: string };
  onDone: (message: string) => void;
  onCancel: () => void;
}

/**
 * A steward's answer to one finding (T-3827): mark it resolved with a note, or correct the
 * attribute's value, which the App writes through its Endpoint, the Policy deciding again.
 */
export function FindingAction({ lang, resolutions, finding, onDone, onCancel }: Props) {
  const [note, setNote] = useState("");
  const [kind, setKind] = useState<Kind>("text");
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const value = valueOf(kind, text);
  const noted = note.trim() !== "";
  const run = async (work: () => Promise<void>, done: Word) => {
    setBusy(true);
    setMessage(null);
    try {
      await work();
      onDone(t(lang, done));
    } catch (error) {
      setMessage(problemText(lang, error));
      setBusy(false);
    }
  };
  const title = t(lang, "actionTitle", { attribute: finding.attribute, id: finding.id });
  return (
    <Card title={title}>
      <form
        className="app-action"
        aria-label={title}
        onSubmit={(event) => {
          event.preventDefault();
          if (noted && !busy) void run(() => resolutions.resolve(finding.entity, finding.attribute, note.trim()), "resolvedDone");
        }}
      >
        <label>
          <span>{t(lang, "note")}</span>
          <input value={note} maxLength={200} required onChange={(event) => setNote(event.target.value)} />
        </label>
        <div className="app-actions">
          <button type="submit" className="jc-button" disabled={!noted || busy}>
            {t(lang, "markResolved")}
          </button>
          <button type="button" className="jc-button" onClick={onCancel}>
            {t(lang, "cancel")}
          </button>
        </div>
        <p className="app-note">{t(lang, "correctLead")}</p>
        <label>
          <span>{t(lang, "valueKind")}</span>
          <select
            value={kind}
            onChange={(event) => {
              setKind(event.target.value as Kind);
              setText(event.target.value === "boolean" ? "true" : "");
            }}
          >
            <option value="text">{t(lang, "kindText")}</option>
            <option value="number">{t(lang, "kindNumber")}</option>
            <option value="boolean">{t(lang, "kindBoolean")}</option>
          </select>
        </label>
        <label>
          <span>{t(lang, "value")}</span>
          {kind === "boolean" ? (
            <select value={text} onChange={(event) => setText(event.target.value)}>
              <option value="true">{t(lang, "yes")}</option>
              <option value="false">{t(lang, "no")}</option>
            </select>
          ) : (
            <input value={text} inputMode={kind === "number" ? "decimal" : undefined} maxLength={500} onChange={(event) => setText(event.target.value)} />
          )}
        </label>
        <button
          type="button"
          className="jc-button"
          disabled={!noted || value === null || busy}
          onClick={() => value !== null && void run(() => resolutions.correct(finding.entity, finding.attribute, value, note.trim()), "correctedDone")}
        >
          {t(lang, "correct")}
        </button>
        <p role="status" aria-live="polite" className={message ? "app-error" : "app-note"}>
          {message ?? ""}
        </p>
      </form>
    </Card>
  );
}
