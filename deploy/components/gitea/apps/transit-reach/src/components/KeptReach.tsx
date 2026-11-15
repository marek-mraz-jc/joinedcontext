import { useState } from "react";
import { go } from "../go";
import { decimal, number, t } from "../i18n";
import type { Lang, Word } from "../i18n";
import { keptReach, ServerProblem } from "../server";
import type { KeptReach as Kept, ReachApi } from "../server";

/** What the server's refusal means to the visitor, in their language. */
export function problemText(lang: Lang, error: unknown): string {
  if (!(error instanceof ServerProblem)) return t(lang, "serverDown");
  const words: Partial<Record<number, Word>> = { 0: "serverOffline", 401: "signInAgain", 403: "noRight", 404: "noSuchStop", 409: "noNetwork", 507: "storageFull" };
  const word = words[error.status];
  if (word) return t(lang, word);
  if (error.status === 400 || error.status === 422) return t(lang, "keptFailed", { why: error.message });
  return t(lang, "serverDown");
}

interface Props {
  lang: Lang;
  api: Pick<ReachApi, "reach" | "refresh">;
  /** The stop the view starts from, its entity id and its label; `null` from any other point. */
  stop: { id: string; label: string } | null;
}

/** The areas from a stop as the server keeps them for HSL's network, and their GeoJSON. */
export function KeptReach({ lang, api, stop }: Props) {
  const [busy, setBusy] = useState(false);
  const [kept, setKept] = useState<{ stop: string; reach: Kept } | null>(null);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);

  const keep = async () => {
    if (!stop) return;
    setBusy(true);
    setMessage(null);
    try {
      const reach = await keptReach(api, stop.id);
      setKept({ stop: stop.label, reach });
      go(reach.url);
      setMessage({ text: t(lang, "keptDownloading"), error: false });
    } catch (error) {
      setMessage({ text: problemText(lang, error), error: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="app-kept" aria-label={t(lang, "kept")}>
      <button type="button" className="jc-button" disabled={!stop || busy} onClick={() => void keep()}>
        {busy ? t(lang, "keeping") : t(lang, "keep")}
      </button>
      <p className="app-note">{stop ? t(lang, "keepFrom", { stop: stop.label }) : t(lang, "keepHint")}</p>
      {kept && (
        <p className="app-note">
          {t(lang, "keptAreas", {
            stop: kept.stop,
            areas: kept.reach.bands.map((band) => t(lang, "keptBand", { minutes: number(lang, band.minutes), km2: decimal(lang, band.areaKm2) })).join(", "),
          })}
        </p>
      )}
      <p role="status" aria-live="polite" className={message?.error ? "app-error" : "app-note"}>
        {message?.text ?? ""}
      </p>
    </section>
  );
}
