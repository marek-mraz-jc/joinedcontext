import { useState } from "react";
import { Card, Empty, Loading } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { localOf, nameOf } from "../events";
import type { Featured } from "../featured";
import type { Lang } from "../i18n";
import { t } from "../texts";
import { problemText } from "./ShareDay";

interface Props {
  lang: Lang;
  featured: Featured;
  /** The events of the day shown, by their full id: a featured one among them can be picked. */
  ofDay: Map<string, Row>;
  /** Every event read, by full id, for the names of featured events on other days. */
  known: Map<string, Row>;
  picks: string[];
  onToggle: (local: string) => void;
}

/**
 * The events a steward recommends, for every visitor: each picked into the day like any other.
 * A steward notes and removes them here, and features one from the list of the day's events.
 */
export function FeaturedCard({ lang, featured, ofDay, known, picks, onToggle }: Props) {
  const { steward } = useAccess();
  // By event: a note typed while the server still features the event (id -1) keeps its draft.
  const [notes, setNotes] = useState<Record<string, string>>({});
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const [busy, setBusy] = useState(false);

  const run = async (work: () => Promise<void>, done: string, failure: "noteFailed" | "unfeatureFailed") => {
    setBusy(true);
    setMessage(null);
    try {
      await work();
      setMessage({ text: done, error: false });
    } catch (error) {
      setMessage({ text: problemText(lang, error, failure, "gone"), error: true });
    } finally {
      setBusy(false);
    }
  };

  const list = featured.list;
  return (
    <Card title={t(lang, "featuredTitle")}>
      {!steward && <p className="app-lead">{t(lang, "featuredWho")}</p>}
      <p role="status" aria-live="polite" className={message?.error ? "app-error" : "app-lead"}>
        {message?.text ?? (featured.failed ? problemText(lang, featured.failed, "featureFailed") : "")}
      </p>
      {list === null ? (
        <Loading label={t(lang, "featuredLoading")} />
      ) : list.length === 0 ? (
        <Empty>{t(lang, "featuredEmpty")}</Empty>
      ) : (
        <ul className="app-featured" aria-label={t(lang, "featuredTitle")}>
          {list.map((one) => {
            const row = ofDay.get(one.eventId) ?? known.get(one.eventId);
            const name = row ? nameOf(row) : localOf(one.eventId);
            const local = localOf(one.eventId);
            const today = ofDay.has(one.eventId);
            const draft = notes[one.eventId] ?? one.note;
            return (
              <li key={one.eventId} className="app-event">
                <div className="app-event-pick">
                  {today ? (
                    <label>
                      <input type="checkbox" checked={picks.includes(local)} onChange={() => onToggle(local)} />
                      <span className="app-sr">
                        {t(lang, "add")}: {name}
                      </span>
                    </label>
                  ) : null}
                  <strong>{name}</strong>
                  {!today && <span className="app-lead"> · {t(lang, "notThisDay")}</span>}
                </div>
                {one.note && !steward && <p>{one.note}</p>}
                {steward && (
                  <form
                    className="app-filters"
                    aria-label={`${t(lang, "note")}: ${name}`}
                    onSubmit={(event) => {
                      event.preventDefault();
                      void run(() => featured.setNote(one.id, draft), t(lang, "noteSaved"), "noteFailed");
                    }}
                  >
                    <label>
                      <span>{t(lang, "note")}</span>
                      <input value={draft} maxLength={200} onChange={(event) => setNotes((all) => ({ ...all, [one.eventId]: event.target.value }))} />
                    </label>
                    <button type="submit" className="jc-button" disabled={busy || one.id < 0 || draft.trim() === one.note}>
                      {t(lang, "saveNote")}
                    </button>
                    <button
                      type="button"
                      className="jc-button"
                      disabled={busy || one.id < 0}
                      onClick={() => void run(() => featured.remove(one.id), t(lang, "unfeatured", { name }), "unfeatureFailed")}
                      aria-label={`${t(lang, "unfeature")}: ${name}`}
                    >
                      {t(lang, "unfeature")}
                    </button>
                  </form>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </Card>
  );
}
