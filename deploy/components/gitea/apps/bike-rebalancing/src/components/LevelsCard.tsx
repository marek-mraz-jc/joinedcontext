import { useId, useState } from "react";
import { Card } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import type { Lang } from "../i18n";
import type { Levels } from "../plans";
import { t } from "../texts";
import { problemText } from "./SavedPlans";

const FIELDS = [
  ["low", "levelLow"],
  ["target", "levelTarget"],
  ["high", "levelHigh"],
] as const;

type Shown = Record<keyof Levels, string>;

const percent = (share: number) => String(Math.round(share * 100));
const shownOf = (levels: Levels): Shown => ({ low: percent(levels.low), target: percent(levels.target), high: percent(levels.high) });

/** Whole percents as shares, when they keep `0 <= low < target < high <= 100`. */
export function levelsOf(shown: Shown): Levels | null {
  const [low, target, high] = [shown.low, shown.target, shown.high].map((text) => (/^\d{1,3}$/.test(text.trim()) ? Number(text) : NaN));
  if (![low, target, high].every(Number.isFinite)) return null;
  if (!(low < target && target < high && high <= 100)) return null;
  return { low: low / 100, target: target / 100, high: high / 100 };
}

/**
 * The planner's levels for every station: a steward changes them, everyone else reads them with the
 * reason the form is closed to them. The form opens once the kept levels arrived, so a steward never
 * edits the defaults. A save shows at once and goes back when the server refuses.
 */
export function LevelsCard({ lang }: { lang: Lang }) {
  const { levels, levelsKept, steward, saveLevels } = useAccess();
  const [shown, setShown] = useState<Shown>(() => shownOf(levels));
  // The levels that `shown` was taken from. New ones replace it in the same render, not in an
  // effect after it: the form opened one commit before the effect ran, and the first value a
  // steward typed went under the kept levels (T-4277).
  const [shownFrom, setShownFrom] = useState(levels);
  if (shownFrom !== levels) {
    setShownFrom(levels);
    setShown(shownOf(levels));
  }
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const why = useId();

  const wanted = levelsOf(shown);
  const closed = !steward || !levelsKept || busy;
  const save = async () => {
    if (!wanted) return;
    const before = levels;
    setBusy(true);
    setMessage(null);
    try {
      await saveLevels(wanted);
      setMessage({ text: t(lang, "levelsSaved"), error: false });
    } catch (error) {
      setShown(shownOf(before));
      setMessage({ text: problemText(lang, error, "levelsFailed"), error: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card title={t(lang, "levelsTitle")}>
      <p className="app-updated">{t(lang, "levelsIntro")}</p>
      <form
        className="app-filters"
        aria-label={t(lang, "levelsTitle")}
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <fieldset disabled={closed} aria-describedby={steward ? undefined : why} title={steward ? undefined : t(lang, "stewardSetsLevels")} className="app-levels">
          {FIELDS.map(([name, label]) => (
            <label key={name}>
              <span>{t(lang, label)}</span>
              <input
                type="number"
                inputMode="numeric"
                min={0}
                max={100}
                step={1}
                disabled={closed}
                value={shown[name]}
                aria-invalid={wanted === null}
                onChange={(event) => setShown((before) => ({ ...before, [name]: event.target.value }))}
              />
            </label>
          ))}
          <button type="submit" className="jc-button" disabled={closed || wanted === null}>
            {t(lang, "levelsSave")}
          </button>
        </fieldset>
        {!steward && (
          <span id={why} className="app-updated">
            {t(lang, "stewardSetsLevels")}
          </span>
        )}
        {steward && wanted === null && <span className="app-error">{t(lang, "levelsOrder")}</span>}
      </form>
      <p role="status" aria-live="polite" className={message?.error ? "app-error" : "app-updated"}>
        {message?.text ?? ""}
      </p>
    </Card>
  );
}
