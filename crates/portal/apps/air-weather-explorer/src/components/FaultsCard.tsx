import { useState } from "react";
import { Card, Empty, Loading } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import type { Faults } from "../faults";
import { helsinkiMs, helsinkiWall } from "../faults";
import { localeOf, number, ZONE } from "../i18n";
import type { Lang } from "../i18n";
import { t } from "../texts";
import type { Key } from "../texts";
import { problemText } from "./Keep";

const HOUR = 3_600_000;

interface Props {
  lang: Lang;
  faults: Faults;
  /** The station the marks are of, its id and name; `null` before one is chosen. */
  station: { id: string; name: string } | null;
  /** The hours the comparison left out for them. */
  leftOut: number;
}

/**
 * The periods a steward marked a station's readings faulty (T-3827), shown to everyone and left
 * out of the comparison; a steward marks one with a reason and takes one back.
 */
export function FaultsCard({ lang, faults, station, leftOut }: Props) {
  const { steward } = useAccess();
  const [now] = useState(() => Date.now());
  const [from, setFrom] = useState(() => helsinkiWall(now - 6 * HOUR));
  const [to, setTo] = useState(() => helsinkiWall(now));
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);

  const when = (ms: number) => new Date(ms).toLocaleString(localeOf(lang), { dateStyle: "short", timeStyle: "short", timeZone: ZONE });
  const run = async (work: () => Promise<void>, done: Key, failed: Key) => {
    setBusy(true);
    setMessage(null);
    try {
      await work();
      setMessage({ text: t(lang, done), error: false });
    } catch (error) {
      setMessage({ text: problemText(lang, error, failed, "gone"), error: true });
    } finally {
      setBusy(false);
    }
  };
  const start = helsinkiMs(from);
  const end = helsinkiMs(to);
  const ready = station !== null && start !== null && end !== null && start < end && note.trim() !== "";

  return (
    <Card title={t(lang, "faultsTitle")}>
      <p className="app-lead">{t(lang, "faultsLead")}</p>
      <p role="status" aria-live="polite" className={message?.error ? "app-error" : "app-lead"}>
        {message?.text ?? (faults.failed ? problemText(lang, faults.failed, "faultsFailed", "gone") : "")}
      </p>
      {faults.list === null ? (
        <Loading label={t(lang, "reading")} />
      ) : faults.list.length === 0 ? (
        faults.failed ? null : <Empty>{t(lang, "faultsNone")}</Empty>
      ) : (
        <ul className="app-faults" aria-label={t(lang, "faultsTitle")}>
          {faults.list.map((fault) => {
            const text = t(lang, "faultLine", { from: when(fault.from), to: when(fault.to), note: fault.note });
            return (
              <li key={fault.id}>
                <span>{text}</span>
                {steward && (
                  <button
                    type="button"
                    className="jc-button"
                    disabled={busy}
                    aria-label={`${t(lang, "unmark")}: ${text}`}
                    onClick={() => void run(() => faults.unmark(fault.id), "unmarked", "unmarkFailed")}
                  >
                    {t(lang, "unmark")}
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {leftOut > 0 && <p className="app-lead">{t(lang, "faultsLeftOut", { n: number(leftOut) })}</p>}
      {steward ? (
        <form
          className="app-filters"
          aria-label={t(lang, "markFaulty")}
          onSubmit={(event) => {
            event.preventDefault();
            if (!ready || start === null || end === null || !station) return;
            void run(
              async () => {
                await faults.mark({ station: station.id, from: start, to: end, note: note.trim() });
                setNote("");
              },
              "marked",
              "markFailed",
            );
          }}
        >
          <label>
            <span>{t(lang, "faultFrom")}</span>
            <input type="datetime-local" value={from} required onChange={(event) => setFrom(event.target.value)} />
          </label>
          <label>
            <span>{t(lang, "faultTo")}</span>
            <input type="datetime-local" value={to} required onChange={(event) => setTo(event.target.value)} />
          </label>
          <label>
            <span>{t(lang, "faultNote")}</span>
            <input value={note} maxLength={200} required onChange={(event) => setNote(event.target.value)} />
          </label>
          <button type="submit" className="jc-button" disabled={busy || !ready}>
            {t(lang, "markFaulty")}
          </button>
        </form>
      ) : (
        <p className="app-lead">{t(lang, "markOnly")}</p>
      )}
    </Card>
  );
}
