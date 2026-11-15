import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Card, Empty, Loading, Page, useEntities } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { problemText } from "../components/Keep";
import { number } from "../i18n";
import type { Lang } from "../i18n";
import type { AirApi, NewPlace, Place } from "../server";
import { AIR, localOf, PLACE_ATTRS, POLLUTANTS, stationsOf } from "../stations";
import type { Pollutant } from "../stations";
import { t } from "../texts";
import type { Key } from "../texts";

/** Where a place opens in the comparison: its station and pollutant in the address, the language kept. */
export function compareHref(place: Pick<Place, "station" | "attr">, lang: Lang): string {
  return `?station=${encodeURIComponent(localOf(place.station))}&air=${place.attr}&lang=${lang}#compare`;
}

/** Where the latest hour stands against the place's level, in words. */
export function standing(lang: Lang, place: Place): { text: string; above: boolean } {
  const level = number(place.threshold, 1);
  if (place.latest === null) return { text: t(lang, "noReading", { level }), above: false };
  const above = place.latest > place.threshold;
  return { text: t(lang, above ? "above" : "below", { value: number(place.latest, 1), level }), above };
}

/**
 * The reader's own places (T-3827): an air quality station, a pollutant and the level above which
 * the latest hourly mean alerts them. Added, changed and deleted by them alone; a change shows at
 * once and goes back when the server refuses.
 */
export function Places({ lang, api }: { lang: Lang; api: AirApi }) {
  const { me } = useAccess();
  const airRows = useEntities(AIR, useMemo(() => ({ attrs: PLACE_ATTRS }), []));
  const stations = useMemo(() => stationsOf(airRows.rows), [airRows.rows]);
  const [list, setShown] = useState<Place[] | null>(null);
  const shown = useRef<Place[]>([]);
  const show = useCallback((next: Place[]) => {
    shown.current = next;
    setShown(next);
  }, []);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const [draft, setDraft] = useState<{ station: string; name: string; attr: Pollutant; threshold: string }>({ station: "", name: "", attr: "pm25", threshold: "25" });
  const [editing, setEditing] = useState<{ id: number; name: string; threshold: string } | null>(null);
  const [confirming, setConfirming] = useState<number | null>(null);

  useEffect(() => {
    if (!me) return;
    let current = true;
    api.places().then(
      (found) => current && show(found),
      (error: unknown) => {
        if (!current) return;
        show([]);
        setMessage({ text: problemText(lang, error, "placesFailed", "gone"), error: true });
      },
    );
    return () => {
      current = false;
    };
  }, [api, me, show, lang]);

  const station = stations.find((s) => s.id === draft.station) ?? stations[0];
  const run = async (work: () => Promise<void>, done: string, failed: Key) => {
    setMessage(null);
    try {
      await work();
      setMessage({ text: done, error: false });
    } catch (error) {
      setMessage({ text: problemText(lang, error, failed, "gone"), error: true });
    }
  };
  const optimistic = async (change: Place[], work: () => Promise<void>) => {
    const before = shown.current;
    show(change);
    try {
      await work();
    } catch (error) {
      show(before);
      throw error;
    }
  };
  const level = (text: string) => (text.trim() === "" ? Number.NaN : Number(text.replace(",", ".")));
  const valid = (name: string, text: string) => name.trim() !== "" && Number.isFinite(level(text)) && level(text) >= 0 && level(text) <= 1000;
  const nameOf = (id: string) => stations.find((s) => s.id === id)?.name ?? localOf(id);

  if (me === null)
    return (
      <Page label={t(lang, "placesPage")}>
        <Card title={t(lang, "placesTitle")}>
          <Empty>{t(lang, "placesSignIn")}</Empty>
        </Card>
      </Page>
    );
  const name = draft.name || station?.name || "";
  return (
    <Page label={t(lang, "placesPage")}>
      <Card title={t(lang, "placesTitle")}>
        <p className="app-lead">{t(lang, "placesLead")}</p>
        <p role="status" aria-live="polite" className={message?.error ? "app-error" : "app-lead"}>
          {message?.text ?? ""}
        </p>
        <form
          className="app-filters"
          aria-label={t(lang, "addPlace")}
          onSubmit={(event) => {
            event.preventDefault();
            if (!station || !valid(name, draft.threshold)) return;
            const place: NewPlace = { station: station.id, name: name.trim(), attr: draft.attr, threshold: level(draft.threshold) };
            void run(
              async () => {
                show([await api.addPlace(place), ...shown.current]);
                setDraft({ ...draft, name: "" });
              },
              t(lang, "added", { name: place.name }),
              "addFailed",
            );
          }}
        >
          <label>
            <span>{t(lang, "station")}</span>
            <select value={station?.id ?? ""} onChange={(event) => setDraft({ ...draft, station: event.target.value, name: "" })}>
              {stations.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>{t(lang, "placeName")}</span>
            <input value={name} maxLength={80} required onChange={(event) => setDraft({ ...draft, name: event.target.value })} />
          </label>
          <label>
            <span>{t(lang, "pollutant")}</span>
            <select value={draft.attr} onChange={(event) => setDraft({ ...draft, attr: event.target.value as Pollutant })}>
              {POLLUTANTS.map((p) => (
                <option key={p} value={p}>
                  {t(lang, p)}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>{t(lang, "placeLevel")}</span>
            <input inputMode="decimal" value={draft.threshold} required onChange={(event) => setDraft({ ...draft, threshold: event.target.value })} />
          </label>
          <button type="submit" className="jc-button" disabled={!station || !valid(name, draft.threshold)}>
            {t(lang, "addPlace")}
          </button>
        </form>
        {list === null ? (
          <Loading label={t(lang, "placesLoading")} />
        ) : list.length === 0 ? (
          <Empty>{t(lang, "placesEmpty")}</Empty>
        ) : (
          <ul className="app-places" aria-label={t(lang, "placesTitle")}>
            {list.map((place) => {
              const state = standing(lang, place);
              return (
                <li key={place.id} className="app-place" data-above={state.above}>
                  {editing?.id === place.id ? (
                    <form
                      className="app-filters"
                      aria-label={`${t(lang, "changePlace")}: ${place.name}`}
                      onSubmit={(event) => {
                        event.preventDefault();
                        const change = { name: editing.name.trim(), threshold: level(editing.threshold) };
                        setEditing(null);
                        void run(
                          () =>
                            optimistic(
                              shown.current.map((one) => (one.id === place.id ? { ...one, ...change } : one)),
                              () => api.changePlace(place.id, change.name, change.threshold),
                            ),
                          t(lang, "changed", { name: change.name }),
                          "changeFailed",
                        );
                      }}
                    >
                      <label>
                        <span>{t(lang, "placeName")}</span>
                        <input value={editing.name} maxLength={80} required onChange={(event) => setEditing({ ...editing, name: event.target.value })} />
                      </label>
                      <label>
                        <span>{t(lang, "placeLevel")}</span>
                        <input inputMode="decimal" value={editing.threshold} required onChange={(event) => setEditing({ ...editing, threshold: event.target.value })} />
                      </label>
                      <button type="submit" className="jc-button" disabled={!valid(editing.name, editing.threshold)}>
                        {t(lang, "saveChange")}
                      </button>
                      <button type="button" className="jc-button" onClick={() => setEditing(null)}>
                        {t(lang, "cancel")}
                      </button>
                    </form>
                  ) : (
                    <p>
                      <strong>{place.name}</strong> · {nameOf(place.station)} · {t(lang, place.attr)}
                    </p>
                  )}
                  <p className={state.above ? "app-error" : "app-lead"}>{state.text}</p>
                  <div className="app-open">
                    <a className="jc-button" href={compareHref(place, lang)} aria-label={`${t(lang, "compareThis")}: ${place.name}`}>
                      {t(lang, "compareThis")}
                    </a>
                    <button
                      type="button"
                      className="jc-button"
                      aria-label={`${t(lang, "changePlace")}: ${place.name}`}
                      onClick={() => setEditing({ id: place.id, name: place.name, threshold: String(place.threshold) })}
                    >
                      {t(lang, "changePlace")}
                    </button>
                    {confirming === place.id ? (
                      <>
                        <button
                          type="button"
                          className="jc-button app-danger"
                          onClick={() => {
                            setConfirming(null);
                            void run(
                              () =>
                                optimistic(
                                  shown.current.filter((one) => one.id !== place.id),
                                  () => api.removePlace(place.id),
                                ),
                              t(lang, "removed", { name: place.name }),
                              "removeFailed",
                            );
                          }}
                        >
                          {t(lang, "confirmRemove")}
                        </button>
                        <button type="button" className="jc-button" onClick={() => setConfirming(null)}>
                          {t(lang, "cancel")}
                        </button>
                      </>
                    ) : (
                      <button type="button" className="jc-button" aria-label={`${t(lang, "remove")}: ${place.name}`} onClick={() => setConfirming(place.id)}>
                        {t(lang, "remove")}
                      </button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </Card>
    </Page>
  );
}
