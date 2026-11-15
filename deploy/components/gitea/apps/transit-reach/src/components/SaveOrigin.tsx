import { useEffect, useState } from "react";
import { Card } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { locale, t } from "../i18n";
import type { Lang } from "../i18n";
import { changeProblem } from "../origins";
import type { KeptBand, ReachApi } from "../server";

interface Props {
  lang: Lang;
  api: ReachApi;
  /** The point the view starts from. */
  at: { lon: number; lat: number };
  /** The stop it starts from, when it is one: the name offered. */
  stopName: string | null;
  /** The areas reached from it and what they rest on; `null` until they are worked out. */
  reach: { source: "network" | "derived"; bands: KeptBand[] } | null;
}

/** Keeps the point the view starts from, with its areas, under a name: the reader's own, compared on the Origins page. */
export function SaveOrigin({ lang, api, at, stopName, reach }: Props) {
  const { me } = useAccess();
  const degrees = (value: number) => new Intl.NumberFormat(locale(lang), { minimumFractionDigits: 4, maximumFractionDigits: 4 }).format(value);
  const offered = stopName ?? t(lang, "pointName", { lat: degrees(at.lat), lon: degrees(at.lon) });
  const [name, setName] = useState(offered);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  // A new starting point offers its own name.
  useEffect(() => setName(offered), [offered]);

  const why = me === null ? t(lang, "saveSignIn") : !reach ? t(lang, "saveWait") : null;
  const keep = async () => {
    if (why || !reach) return;
    setBusy(true);
    setMessage(null);
    try {
      const kept = await api.keep({ name: name.trim(), lon: at.lon, lat: at.lat, source: reach.source, bands: reach.bands });
      setMessage({ text: t(lang, "saved", { name: kept.name }), error: false });
    } catch (error) {
      setMessage({ text: changeProblem(lang, error, "saveFailed"), error: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card title={t(lang, "saveTitle")}>
      <p className="app-note">{t(lang, "saveHint")}</p>
      <form
        className="app-filters"
        aria-label={t(lang, "saveTitle")}
        onSubmit={(event) => {
          event.preventDefault();
          void keep();
        }}
      >
        <label>
          <span>{t(lang, "saveName")}</span>
          <input value={name} maxLength={80} required onChange={(event) => setName(event.target.value)} disabled={me === null} />
        </label>
        <button type="submit" className="jc-button" disabled={busy || why !== null || name.trim() === ""} title={why ?? undefined}>
          {busy ? t(lang, "saving") : t(lang, "save")}
        </button>
      </form>
      {why && me !== undefined && <p className="app-note">{why}</p>}
      <p role="status" aria-live="polite" className={message?.error ? "app-error" : "app-note"}>
        {message?.text ?? ""}
      </p>
    </Card>
  );
}
