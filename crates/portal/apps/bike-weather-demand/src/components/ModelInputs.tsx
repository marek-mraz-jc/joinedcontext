import { useEffect, useState } from "react";
import { Card } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { stationName } from "../bikes";
import { moment, number, t } from "../i18n";
import type { Lang } from "../i18n";
import type { Inputs, NewInputs, Server } from "../server";

const reason = (error: unknown) => (error instanceof Error ? error.message : String(error));

const draftOf = (inputs: Inputs) => ({ weatherStation: inputs.weatherStation ?? "", radiusKm: String(inputs.radiusKm), days: String(inputs.days), note: inputs.note });

/** The draft as the server would take it, or the word that says what is wrong. */
export function checkedDraft(draft: { weatherStation: string; radiusKm: string; days: string; note: string }): NewInputs | "radiusRange" | "daysRange" | "noteLong" {
  const radiusKm = Number(draft.radiusKm.replace(",", "."));
  const days = Number(draft.days);
  if (!draft.radiusKm.trim() || !Number.isFinite(radiusKm) || radiusKm < 1 || radiusKm > 25) return "radiusRange";
  if (!Number.isInteger(days) || days < 1 || days > 7) return "daysRange";
  if (draft.note.trim().length > 200) return "noteLong";
  return { weatherStation: draft.weatherStation || null, radiusKm, days, note: draft.note.trim() };
}

/**
 * A station's model inputs (T-3827): which weather station its model reads, how far the nearest may
 * be, and how many days it trains on. Everyone reads them; a steward changes them, saved at once on
 * screen and taken back when the server refuses; today's model is trained again on them.
 */
export function ModelInputs({
  lang,
  server,
  station,
  weatherStations,
  inputs,
  failed,
  onInputs,
  onSaved,
}: {
  lang: Lang;
  server: Server;
  station: string;
  weatherStations: Row[];
  inputs: Inputs | null;
  /** Why the inputs could not be read. */
  failed: string | null;
  onInputs: (inputs: Inputs) => void;
  onSaved: () => void;
}) {
  const { me, steward } = useAccess();
  const [draft, setDraft] = useState({ weatherStation: "", radiusKm: "10", days: "7", note: "" });
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);

  // The form takes the inputs as read, and as saved; a refused save leaves what was typed, to try again.
  const loaded = inputs === null ? null : inputs.station;
  useEffect(() => {
    if (inputs) setDraft(draftOf(inputs));
  }, [loaded]);
  useEffect(() => {
    setProblem(null);
    setDone(null);
  }, [station]);

  // The reason a control is closed, as its tooltip: never a silent no-op.
  const closed = steward ? undefined : t(lang, me ? "inputsStewardOnly" : "inputsSignIn");
  const checked = checkedDraft(draft);

  const change = async (next: Inputs, write: () => Promise<Inputs>, word: "inputsSaved" | "inputsReset") => {
    if (!inputs) return;
    const before = inputs;
    setBusy(true);
    setProblem(null);
    setDone(null);
    onInputs(next);
    try {
      const saved = await write();
      onInputs(saved);
      setDraft(draftOf(saved));
      setDone(t(lang, word));
      onSaved();
    } catch (error) {
      onInputs(before);
      setProblem(t(lang, "inputsRefused", { reason: reason(error) }));
    } finally {
      setBusy(false);
    }
  };

  const save = () => {
    if (typeof checked === "string" || !inputs) return;
    void change(
      { ...inputs, ...checked, custom: true, updatedBy: me?.name ?? null, updatedAt: new Date().toISOString() },
      () => server.setInputs(station, checked),
      "inputsSaved",
    );
  };
  const reset = () => {
    if (!inputs) return;
    void change(
      { station, weatherStation: null, radiusKm: 10, days: 7, note: "", custom: false, updatedBy: null, updatedAt: null },
      async () => {
        await server.resetInputs(station);
        return server.inputs(station);
      },
      "inputsReset",
    );
  };

  const nameOf = (id: string) => {
    const found = weatherStations.find((w) => w.id === id);
    return found ? stationName(found) : id;
  };
  const updated = inputs?.updatedAt ? Date.parse(inputs.updatedAt) : NaN;

  return (
    <Card title={t(lang, "inputsTitle")}>
      {failed ? (
        <p className="jc-problem" role="alert">
          {t(lang, "inputsFailed", { reason: failed })}
        </p>
      ) : !inputs ? (
        <p className="app-note">{t(lang, "inputsReading")}</p>
      ) : (
        <>
          <p className="app-note" data-testid="inputs-now">
            {t(lang, "inputsNow", {
              weather: inputs.weatherStation ? nameOf(inputs.weatherStation) : t(lang, "inputsNearest", { km: number(lang, inputs.radiusKm, 1) }),
              days: inputs.days,
            })}{" "}
            {inputs.custom
              ? t(lang, "inputsSetBy", { who: inputs.updatedBy ?? "–", at: Number.isNaN(updated) ? "–" : moment(lang, updated) })
              : t(lang, "inputsDefaults")}
          </p>
          {inputs.custom && inputs.note && <blockquote className="app-quote">{inputs.note}</blockquote>}
          <form
            className="app-inputs"
            aria-label={t(lang, "inputsTitle")}
            onSubmit={(event) => {
              event.preventDefault();
              save();
            }}
          >
            <p className="app-note">{steward ? t(lang, "inputsLead") : closed}</p>
            <label>
              <span>{t(lang, "inputsWeather")}</span>
              <select
                value={draft.weatherStation}
                disabled={!steward || busy}
                title={closed}
                onChange={(event) => setDraft({ ...draft, weatherStation: event.target.value })}
              >
                <option value="">{t(lang, "inputsNearestOption")}</option>
                {weatherStations.map((w) => (
                  <option key={w.id} value={w.id}>
                    {stationName(w)}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span>{t(lang, "inputsRadius")}</span>
              <input
                inputMode="decimal"
                value={draft.radiusKm}
                disabled={!steward || busy}
                title={closed}
                aria-invalid={checked === "radiusRange"}
                onChange={(event) => setDraft({ ...draft, radiusKm: event.target.value })}
              />
            </label>
            <label>
              <span>{t(lang, "inputsDays")}</span>
              <input
                inputMode="numeric"
                value={draft.days}
                disabled={!steward || busy}
                title={closed}
                aria-invalid={checked === "daysRange"}
                onChange={(event) => setDraft({ ...draft, days: event.target.value })}
              />
            </label>
            <label>
              <span>{t(lang, "inputsNote")}</span>
              <input
                value={draft.note}
                maxLength={200}
                disabled={!steward || busy}
                title={closed}
                onChange={(event) => setDraft({ ...draft, note: event.target.value })}
              />
            </label>
            {steward && typeof checked === "string" && (
              <p className="app-error" role="alert">
                {t(lang, checked)}
              </p>
            )}
            <div className="app-inputs-actions">
              <button type="submit" className="jc-button" disabled={!steward || busy || typeof checked === "string"} title={closed}>
                {t(lang, "inputsSave")}
              </button>
              <button
                type="button"
                className="jc-button"
                disabled={!steward || busy || !inputs.custom}
                title={closed ?? (inputs.custom ? undefined : t(lang, "inputsAlreadyDefaults"))}
                onClick={reset}
              >
                {t(lang, "inputsResetButton")}
              </button>
            </div>
          </form>
          {problem && (
            <p className="app-error" role="alert">
              {problem}
            </p>
          )}
          {done && (
            <p className="app-note" role="status">
              {done}
            </p>
          )}
        </>
      )}
    </Card>
  );
}
