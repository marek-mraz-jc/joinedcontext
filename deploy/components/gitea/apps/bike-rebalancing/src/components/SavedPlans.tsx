import { useCallback, useEffect, useId, useState } from "react";
import { Card, Empty, Loading } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { localeOf, number, ZONE } from "../i18n";
import type { Lang } from "../i18n";
import { go } from "../go";
import { ServerProblem } from "../plans";
import type { NewPlan, PlanSummary, PlansApi, SavedPlan } from "../plans";
import { t } from "../texts";
import type { Key } from "../texts";
import { useParam } from "../url";

/** What the server's refusal means to the operator, in their language. */
export function problemText(lang: Lang, error: unknown, action: Key): string {
  if (!(error instanceof ServerProblem)) return t(lang, "serverDown");
  if (error.status === 0) return t(lang, "serverOffline");
  if (error.status === 401) return t(lang, "signInAgain");
  if (error.status === 403) return t(lang, "noRight");
  if (error.status === 400 || error.status === 409) return `${t(lang, action)}: ${error.message}`;
  if (error.status === 404) return t(lang, "planGone");
  if (error.status === 507) return t(lang, "storageFull");
  return t(lang, "serverDown");
}

interface Props {
  lang: Lang;
  api: PlansApi;
  /** The choices on screen, saved as a plan. */
  current: Omit<NewPlan, "operator">;
  /** A saved plan's choices put back on screen. */
  onOpen: (plan: SavedPlan) => void;
}

/**
 * The plans kept on the server, of one operator (a van or a crew): save the plan on screen, open
 * one again, download its route sheet, record that it was driven, delete it. A viewer sees their
 * own plans; a steward sees everyone's and hands a plan to another van or crew.
 */
export function SavedPlans({ lang, api, current, onOpen }: Props) {
  const [operator, setOperator] = useParam("op");
  const [plans, setPlans] = useState<PlanSummary[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const [confirming, setConfirming] = useState<number | null>(null);
  const [handing, setHanding] = useState<{ id: number; to: string } | null>(null);
  const { me, steward } = useAccess();
  const operatorId = useId();
  const handId = useId();

  const load = useCallback(async () => {
    try {
      setPlans(await api.list(operator));
    } catch (error) {
      setPlans([]);
      setMessage({ text: problemText(lang, error, "loadFailed"), error: true });
    }
  }, [api, operator, lang]);

  useEffect(() => {
    void load();
  }, [load]);

  const run = async (what: string, work: () => Promise<string>, failure: Key) => {
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
        const saved = await api.save({ ...current, operator: operator.trim() });
        await load();
        return t(lang, "saved", { n: number(saved.stops.length), op: saved.operator });
      },
      "saveFailed",
    );
  const open = (id: number) =>
    run(
      `open-${id}`,
      async () => {
        const plan = await api.get(id);
        onOpen(plan);
        return t(lang, "opened", { id: number(id) });
      },
      "openFailed",
    );
  const sheet = (id: number) =>
    run(
      `sheet-${id}`,
      async () => {
        const url = await api.sheetUrl(id);
        go(url);
        return t(lang, "sheetReady");
      },
      "sheetFailed",
    );
  const driven = (id: number) =>
    run(
      `drive-${id}`,
      async () => {
        const plan = await api.get(id);
        await api.drive(id, plan.stops.map((stop) => stop.id));
        await load();
        return t(lang, "drivenSaved");
      },
      "driveFailed",
    );
  /** Shown at once; the list goes back as it was when the server refuses. */
  const handOver = (id: number, to: string) =>
    run(
      `hand-${id}`,
      async () => {
        const before = plans;
        setPlans((list) => list?.map((plan) => (plan.id === id ? { ...plan, operator: to.trim() } : plan)) ?? list);
        try {
          await api.handOver(id, to.trim());
        } catch (error) {
          setPlans(before);
          throw error;
        }
        setHanding(null);
        return t(lang, "handedOver", { op: to.trim() });
      },
      "handOverFailed",
    );
  const remove = (id: number) =>
    run(
      `delete-${id}`,
      async () => {
        await api.remove(id);
        setConfirming(null);
        await load();
        return t(lang, "deleted");
      },
      "deleteFailed",
    );

  const named = operator.trim().length > 0;
  const signedOut = me === null;
  const when = (iso: string) => {
    const at = new Date(iso);
    return Number.isNaN(at.getTime()) ? iso : at.toLocaleString(localeOf(lang), { dateStyle: "medium", timeStyle: "short", timeZone: ZONE });
  };

  return (
    <Card title={t(lang, "savedPlans")}>
      <form
        className="app-filters"
        aria-label={t(lang, "savedPlans")}
        onSubmit={(event) => {
          event.preventDefault();
          if (named && !signedOut && busy === null) void save();
        }}
      >
        <label htmlFor={operatorId}>
          <span>{t(lang, "operator")}</span>
        </label>
        <input id={operatorId} value={operator} maxLength={60} placeholder={t(lang, "operatorHint")} onChange={(event) => setOperator(event.target.value)} />
        <button
          type="submit"
          className="jc-button"
          disabled={!named || signedOut || busy !== null}
          aria-describedby={named && !signedOut ? undefined : `${operatorId}-why`}
          title={signedOut ? t(lang, "signInToSave") : undefined}
        >
          {busy === "save" ? t(lang, "saving") : t(lang, "savePlan")}
        </button>
        {(signedOut || !named) && (
          <span id={`${operatorId}-why`} className="app-updated">
            {t(lang, signedOut ? "signInToSave" : "operatorFirst")}
          </span>
        )}
      </form>
      <p role="status" aria-live="polite" className={message?.error ? "app-error" : "app-updated"}>
        {message?.text ?? ""}
      </p>
      {plans === null ? (
        <Loading label={t(lang, "loadingPlans")} />
      ) : plans.length === 0 ? (
        <Empty>{named ? t(lang, "noPlansOf", { op: operator.trim() }) : t(lang, "noPlans")}</Empty>
      ) : (
        <>
        {!steward && (
          <p id={handId} className="app-updated">
            {t(lang, "stewardHandsOver")}
          </p>
        )}
        <ul className="app-plans" aria-label={t(lang, "savedPlans")}>
          {plans.map((plan) => (
            <li key={plan.id} className="app-plan">
              <div>
                <strong>
                  {plan.operator} · {when(plan.createdAt)}
                  {!plan.mine && <span className="app-updated"> · {t(lang, "someoneElses")}</span>}
                </strong>
                <p>{t(lang, "planLine", { moved: number(plan.moved), km: number(plan.km, 1), stops: number(plan.stopCount), drives: number(plan.drives) })}</p>
              </div>
              <div className="app-plan-actions">
                <button type="button" className="jc-button" disabled={busy !== null} onClick={() => void open(plan.id)} aria-label={`${t(lang, "openPlan")}: ${plan.operator}, ${when(plan.createdAt)}`}>
                  {t(lang, "openPlan")}
                </button>
                <button type="button" className="jc-button" disabled={busy !== null} onClick={() => void sheet(plan.id)} aria-label={`${t(lang, "routeSheet")}: ${plan.operator}, ${when(plan.createdAt)}`}>
                  {t(lang, "routeSheet")}
                </button>
                <button type="button" className="jc-button" disabled={busy !== null || plan.stopCount === 0} onClick={() => void driven(plan.id)} aria-label={`${t(lang, "markDriven")}: ${plan.operator}, ${when(plan.createdAt)}`}>
                  {t(lang, "markDriven")}
                </button>
                {handing?.id === plan.id ? (
                  <form
                    className="app-filters"
                    aria-label={`${t(lang, "handOver")}: ${plan.operator}`}
                    onSubmit={(event) => {
                      event.preventDefault();
                      if (handing.to.trim()) void handOver(plan.id, handing.to);
                    }}
                  >
                    <label>
                      <span>{t(lang, "handOverTo")}</span>
                      <input value={handing.to} maxLength={60} autoFocus onChange={(event) => setHanding({ id: plan.id, to: event.target.value })} />
                    </label>
                    <button type="submit" className="jc-button" disabled={busy !== null || !handing.to.trim()}>
                      {t(lang, "handOverSave")}
                    </button>
                    <button type="button" className="jc-button" onClick={() => setHanding(null)}>
                      {t(lang, "cancel")}
                    </button>
                  </form>
                ) : (
                  <button
                    type="button"
                    className="jc-button"
                    disabled={busy !== null || !steward}
                    title={steward ? undefined : t(lang, "stewardHandsOver")}
                    aria-describedby={steward ? undefined : handId}
                    onClick={() => setHanding({ id: plan.id, to: plan.operator })}
                    aria-label={`${t(lang, "handOver")}: ${plan.operator}, ${when(plan.createdAt)}`}
                  >
                    {t(lang, "handOver")}
                  </button>
                )}
                {confirming === plan.id ? (
                  <>
                    <button type="button" className="jc-button app-danger" disabled={busy !== null} onClick={() => void remove(plan.id)}>
                      {t(lang, "confirmDelete")}
                    </button>
                    <button type="button" className="jc-button" onClick={() => setConfirming(null)}>
                      {t(lang, "cancel")}
                    </button>
                  </>
                ) : (
                  <button type="button" className="jc-button" disabled={busy !== null} onClick={() => setConfirming(plan.id)} aria-label={`${t(lang, "deletePlan")}: ${plan.operator}, ${when(plan.createdAt)}`}>
                    {t(lang, "deletePlan")}
                  </button>
                )}
              </div>
            </li>
          ))}
        </ul>
        </>
      )}
    </Card>
  );
}
