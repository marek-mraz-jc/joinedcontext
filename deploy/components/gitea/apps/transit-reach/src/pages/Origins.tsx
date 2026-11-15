import { useState } from "react";
import { Card, Empty, Loading, Page } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { decimal, number, t } from "../i18n";
import type { Lang, Word } from "../i18n";
import { changeProblem, useOrigins } from "../origins";
import type { Origin, ReachApi } from "../server";

const BANDS = [10, 20, 30];

/** The view of an origin: its point in the address, the language kept, on the Reach page. */
export function openHref(origin: Pick<Origin, "lon" | "lat">, lang: Lang): string {
  return `?at=${origin.lon.toFixed(5)},${origin.lat.toFixed(5)}&lang=${lang}#reach`;
}

/**
 * The origins side by side (T-3826): the area reached from each in 10, 20 and 30 minutes and the
 * stops within 30. The reader's own, renamed or deleted by them; the published ones a steward
 * chose for everyone; a steward sees and changes every one and publishes or withdraws it.
 */
export function Origins({ lang, api }: { lang: Lang; api: ReachApi }) {
  const { me, steward } = useAccess();
  const origins = useOrigins(api, me !== undefined);
  const [editing, setEditing] = useState<{ id: number; name: string } | null>(null);
  const [confirming, setConfirming] = useState<number | null>(null);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);

  const run = async (work: () => Promise<void>, done: string, failed: Word) => {
    setMessage(null);
    try {
      await work();
      setMessage({ text: done, error: false });
    } catch (error) {
      setMessage({ text: changeProblem(lang, error, failed), error: true });
    }
  };
  const area = (origin: Origin, minutes: number) => {
    const band = origin.bands.find((b) => b.minutes === minutes);
    return band ? t(lang, "area", { km2: decimal(lang, band.areaKm2) }) : "–";
  };
  const list = origins.list;

  return (
    <Page label={t(lang, "originsPage")}>
      <Card title={t(lang, "originsTitle")}>
        <p className="app-note">{t(lang, steward ? "originsLeadSteward" : "originsLead")}</p>
        <p role="status" aria-live="polite" className={message?.error ? "app-error" : "app-note"}>
          {message?.text ?? (origins.failed ? changeProblem(lang, origins.failed, "originsFailed") : "")}
        </p>
        {list === null ? (
          <Loading label={t(lang, "originsLoading")} />
        ) : list.length === 0 ? (
          origins.failed ? null : <Empty>{t(lang, "originsEmpty")}</Empty>
        ) : (
          <div className="app-table">
            <table>
              <caption className="app-sr">{t(lang, "originsTitle")}</caption>
              <thead>
                <tr>
                  <th scope="col">{t(lang, "colName")}</th>
                  {BANDS.map((minutes) => (
                    <th scope="col" key={minutes}>
                      {t(lang, "colMinutes", { minutes: number(lang, minutes) })}
                    </th>
                  ))}
                  <th scope="col">{t(lang, "colStops")}</th>
                  <th scope="col">{t(lang, "colState")}</th>
                  <th scope="col">{t(lang, "colActions")}</th>
                </tr>
              </thead>
              <tbody>
                {list.map((origin) => {
                  const changeable = origin.mine || steward;
                  const why = changeable ? undefined : t(lang, "notYours");
                  const state = [origin.mine ? t(lang, "stateMine") : steward ? t(lang, "stateOther") : null, origin.published ? t(lang, "statePublished") : null]
                    .filter(Boolean)
                    .join(", ");
                  return (
                    <tr key={origin.id}>
                      <th scope="row">
                        {editing?.id === origin.id ? (
                          <form
                            className="app-filters"
                            aria-label={`${t(lang, "rename")}: ${origin.name}`}
                            onSubmit={(event) => {
                              event.preventDefault();
                              const name = editing.name;
                              setEditing(null);
                              void run(() => origins.rename(origin.id, name), t(lang, "renamed", { name: name.trim() }), "renameFailed");
                            }}
                          >
                            <label>
                              <span>{t(lang, "saveName")}</span>
                              <input value={editing.name} maxLength={80} required onChange={(event) => setEditing({ id: origin.id, name: event.target.value })} />
                            </label>
                            <button type="submit" className="jc-button" disabled={editing.name.trim() === "" || editing.name.trim() === origin.name}>
                              {t(lang, "renameSave")}
                            </button>
                            <button type="button" className="jc-button" onClick={() => setEditing(null)}>
                              {t(lang, "cancel")}
                            </button>
                          </form>
                        ) : (
                          origin.name
                        )}
                      </th>
                      {BANDS.map((minutes) => (
                        <td key={minutes}>{area(origin, minutes)}</td>
                      ))}
                      <td>{number(lang, origin.bands.find((b) => b.minutes === 30)?.stops ?? 0)}</td>
                      <td>{state}</td>
                      <td>
                        <div className="app-actions">
                          <a className="jc-button" href={openHref(origin, lang)} aria-label={`${t(lang, "open")}: ${origin.name}`}>
                            {t(lang, "open")}
                          </a>
                          {me && (
                            <>
                              <button
                                type="button"
                                className="jc-button"
                                disabled={!changeable}
                                title={why}
                                aria-label={`${t(lang, "rename")}: ${origin.name}`}
                                onClick={() => setEditing({ id: origin.id, name: origin.name })}
                              >
                                {t(lang, "rename")}
                              </button>
                              <button
                                type="button"
                                className="jc-button"
                                disabled={!steward}
                                title={steward ? undefined : t(lang, "publishOnly")}
                                aria-label={`${t(lang, origin.published ? "withdraw" : "publish")}: ${origin.name}`}
                                onClick={() =>
                                  void run(
                                    () => origins.publish(origin.id, !origin.published),
                                    t(lang, origin.published ? "withdrawn" : "published", { name: origin.name }),
                                    "publishFailed",
                                  )
                                }
                              >
                                {t(lang, origin.published ? "withdraw" : "publish")}
                              </button>
                              {confirming === origin.id ? (
                                <>
                                  <button
                                    type="button"
                                    className="jc-button app-danger"
                                    onClick={() => {
                                      setConfirming(null);
                                      void run(() => origins.remove(origin.id), t(lang, "removed", { name: origin.name }), "removeFailed");
                                    }}
                                  >
                                    {t(lang, "confirmRemove")}
                                  </button>
                                  <button type="button" className="jc-button" onClick={() => setConfirming(null)}>
                                    {t(lang, "cancel")}
                                  </button>
                                </>
                              ) : (
                                <button
                                  type="button"
                                  className="jc-button"
                                  disabled={!changeable}
                                  title={why}
                                  aria-label={`${t(lang, "remove")}: ${origin.name}`}
                                  onClick={() => setConfirming(origin.id)}
                                >
                                  {t(lang, "remove")}
                                </button>
                              )}
                            </>
                          )}
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
        {me && !steward && <p className="app-note">{t(lang, "publishOnly")}</p>}
      </Card>
    </Page>
  );
}
