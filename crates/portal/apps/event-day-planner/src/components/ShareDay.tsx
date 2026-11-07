import { useId, useState } from "react";
import { go } from "../go";
import type { Lang } from "../i18n";
import { ServerProblem, shareLink } from "../share";
import type { ShareApi } from "../share";
import { t } from "../texts";
import type { Key } from "../texts";

/** What the server's refusal means to the visitor, in their language; a `404` reads as `gone`. */
export function problemText(lang: Lang, error: unknown, action: Key, gone: Key = "shareGone"): string {
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
  api: ShareApi;
  /** The Helsinki day shown, `YYYY-MM-DD`. */
  day: string;
  /** The picked events' full ids. */
  ids: string[];
}

/**
 * Shares the picked day: the server plans it again from the city's data and keeps it under a
 * code; the visitor gets the link to copy and the calendar file of what was shared.
 */
export function ShareDay({ lang, api, day, ids }: Props) {
  const [busy, setBusy] = useState(false);
  const [code, setCode] = useState<string | null>(null);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const linkId = useId();

  const share = async () => {
    setBusy(true);
    setMessage(null);
    try {
      const shared = await api.share(day, ids, lang);
      setCode(shared.code);
      setMessage({ text: t(lang, "shared", { n: shared.items.length }), error: false });
    } catch (error) {
      setMessage({ text: problemText(lang, error, "shareFailed"), error: true });
    } finally {
      setBusy(false);
    }
  };
  const copy = async () => {
    if (!code) return;
    try {
      await navigator.clipboard.writeText(shareLink(code));
      setMessage({ text: t(lang, "copied"), error: false });
    } catch {
      (document.getElementById(linkId) as HTMLInputElement | null)?.select();
      setMessage({ text: t(lang, "copyByHand"), error: false });
    }
  };
  const calendar = async () => {
    if (!code) return;
    try {
      go(await api.icsUrl(code));
    } catch (error) {
      setMessage({ text: problemText(lang, error, "icsFailed"), error: true });
    }
  };

  return (
    <div className="app-share">
      <button type="button" className="jc-button" onClick={() => void share()} disabled={busy || ids.length === 0} title={ids.length === 0 ? t(lang, "pickFirst") : undefined}>
        {busy ? t(lang, "sharing") : t(lang, "share")}
      </button>
      {ids.length === 0 && <p className="app-lead">{t(lang, "pickFirst")}</p>}
      {code && (
        <div className="app-share-link">
          <label htmlFor={linkId}>{t(lang, "shareLink")}</label>
          <input id={linkId} readOnly value={shareLink(code)} onFocus={(event) => event.currentTarget.select()} />
          <button type="button" className="jc-button" onClick={() => void copy()}>
            {t(lang, "copy")}
          </button>
          <button type="button" className="jc-button" onClick={() => void calendar()}>
            {t(lang, "sharedIcs")}
          </button>
        </div>
      )}
      <p role="status" aria-live="polite" className={message?.error ? "app-error" : "app-lead"}>
        {message?.text ?? ""}
      </p>
    </div>
  );
}
