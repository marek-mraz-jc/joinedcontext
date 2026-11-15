import { useId, useState } from "react";
import { go } from "../go";
import type { Lang } from "../i18n";
import { compareLink, ServerProblem } from "../server";
import type { AirApi, Comparison } from "../server";
import { t } from "../texts";
import type { Key } from "../texts";

/** What the server's refusal means to the reader, in their language; a `404` reads as `gone`. */
export function problemText(lang: Lang, error: unknown, action: Key, gone: Key = "compareGone"): string {
  if (!(error instanceof ServerProblem)) return t(lang, "serverDown");
  if (error.status === 0) return t(lang, "serverOffline");
  if (error.status === 401) return t(lang, "signInAgain");
  if (error.status === 403) return t(lang, "noRight");
  if (error.status === 400 || error.status === 409) return `${t(lang, action)}: ${error.message}`;
  if (error.status === 404) return t(lang, gone);
  if (error.status === 507) return t(lang, "storageFull");
  return t(lang, "serverDown");
}

interface Props {
  lang: Lang;
  api: AirApi;
  /** The comparison on screen; `null` until both stations are known. */
  comparison: Omit<Comparison, "name"> | null;
}

/** Keeps the comparison on screen under a link, and gives its hours as CSV. */
export function Keep({ lang, api, comparison }: Props) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState<"save" | "export" | null>(null);
  const [code, setCode] = useState<string | null>(null);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const nameId = useId();
  const linkId = useId();

  const run = async (what: "save" | "export", work: () => Promise<string>, failure: Key) => {
    setBusy(what);
    setMessage(null);
    try {
      setMessage({ text: await work(), error: false });
    } catch (error) {
      setMessage({ text: problemText(lang, error, failure), error: true });
    } finally {
      setBusy(null);
    }
  };
  const save = () =>
    run(
      "save",
      async () => {
        if (!comparison) return "";
        const saved = await api.save({ ...comparison, name: name.trim() || null });
        setCode(saved.code);
        return t(lang, "saved");
      },
      "saveFailed",
    );
  const exportCsv = () =>
    run(
      "export",
      async () => {
        if (!comparison) return "";
        go(await api.exportUrl(comparison.station, comparison.weather, comparison.days));
        return t(lang, "exporting");
      },
      "exportFailed",
    );
  const copy = async () => {
    if (!code) return;
    try {
      await navigator.clipboard.writeText(compareLink(code));
      setMessage({ text: t(lang, "copied"), error: false });
    } catch {
      (document.getElementById(linkId) as HTMLInputElement | null)?.select();
      setMessage({ text: t(lang, "copyByHand"), error: false });
    }
  };

  return (
    <section className="app-keep" aria-label={t(lang, "keep")}>
      <form
        className="app-filters"
        aria-label={t(lang, "keep")}
        onSubmit={(event) => {
          event.preventDefault();
          if (comparison && busy === null) void save();
        }}
      >
        <label htmlFor={nameId}>
          <span>{t(lang, "compareName")}</span>
        </label>
        <input id={nameId} value={name} maxLength={80} placeholder={t(lang, "compareNameHint")} onChange={(event) => setName(event.target.value)} />
        <button type="submit" className="jc-button" disabled={!comparison || busy !== null}>
          {busy === "save" ? t(lang, "saving") : t(lang, "save")}
        </button>
        <button type="button" className="jc-button" disabled={!comparison || busy !== null} onClick={() => void exportCsv()}>
          {busy === "export" ? t(lang, "exportBusy") : t(lang, "export")}
        </button>
      </form>
      {code && (
        <div className="app-keep-link">
          <label htmlFor={linkId}>{t(lang, "compareLink")}</label>
          <input id={linkId} readOnly value={compareLink(code)} onFocus={(event) => event.currentTarget.select()} />
          <button type="button" className="jc-button" onClick={() => void copy()}>
            {t(lang, "copy")}
          </button>
        </div>
      )}
      <p role="status" aria-live="polite" className={message?.error ? "app-error" : "app-lead"}>
        {message?.text ?? ""}
      </p>
    </section>
  );
}
