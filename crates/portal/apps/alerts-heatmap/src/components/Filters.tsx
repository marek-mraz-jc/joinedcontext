import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { Card, Empty, Loading } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { t } from "../i18n";
import type { Lang } from "../i18n";
import type { Filter, Server } from "../server";

// The server and fetch fail with an Error each.
const reason = (error: unknown) => (error as Error).message;

/**
 * The reader's own filters (T-3828): the filters on screen kept under a name on the App's server
 * and applied again with a click. Shown at once, taken back when the server refuses.
 */
export function Filters({ lang, server, onApply }: { lang: Lang; server: Server; onApply: (view: string) => void }) {
  const { me } = useAccess();
  const [filters, setFilters] = useState<Filter[]>([]);
  const [ready, setReady] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [said, setSaid] = useState<{ text: string; problem: boolean } | null>(null);

  useEffect(() => {
    if (!me) return;
    let gone = false;
    server
      .filters()
      .then(
        (answer) => !gone && setFilters(answer),
        (error: unknown) => !gone && setFailed(reason(error)),
      )
      .finally(() => setReady(true));
    return () => {
      gone = true;
    };
  }, [server, me]);

  const keep = async (event: FormEvent) => {
    event.preventDefault();
    const named = name.trim();
    if (!named || named.length > 80) {
      setSaid({ text: t(lang, "filterNeedsName"), problem: true });
      return;
    }
    const view = window.location.search.replace(/^\?/, "");
    const draft: Filter = { id: -Date.now(), name: named, view };
    setFilters((current) => [draft, ...current]);
    setName("");
    setSaid(null);
    try {
      const kept = await server.keepFilter({ name: named, view });
      setFilters((current) => current.map((filter) => (filter.id === draft.id ? kept : filter)));
      setSaid({ text: t(lang, "filterKept", { name: kept.name }), problem: false });
    } catch (error) {
      setFilters((current) => current.filter((filter) => filter.id !== draft.id));
      setName(named);
      setSaid({ text: t(lang, "refused", { reason: reason(error) }), problem: true });
    }
  };

  const forget = async (filter: Filter) => {
    setFilters((current) => current.filter((other) => other.id !== filter.id));
    setSaid(null);
    try {
      await server.forgetFilter(filter.id);
      setSaid({ text: t(lang, "filterForgotten", { name: filter.name }), problem: false });
    } catch (error) {
      setFilters((current) => [filter, ...current]);
      setSaid({ text: t(lang, "refused", { reason: reason(error) }), problem: true });
    }
  };

  const signedOut = me === null ? t(lang, "filtersSignIn") : undefined;
  return (
    <Card title={t(lang, "myFilters")}>
      <p className="app-note">{t(lang, "myFiltersNote")}</p>
      <form className="app-save" onSubmit={(event) => void keep(event)}>
        <label>
          <span>{t(lang, "filterName")}</span>
          <input value={name} maxLength={80} disabled={!me} title={signedOut} onChange={(event) => setName(event.target.value)} />
        </label>
        <button type="submit" className="jc-button" disabled={!me} title={signedOut}>
          {t(lang, "keepFilter")}
        </button>
      </form>
      {said && (
        <p className={said.problem ? "jc-problem" : "app-note"} role={said.problem ? "alert" : "status"}>
          {said.text}
        </p>
      )}
      {failed && <p className="jc-problem" role="alert">{t(lang, "filtersFailed", { reason: failed })}</p>}
      {me === null ? (
        <Empty>{t(lang, "filtersSignIn")}</Empty>
      ) : !ready ? (
        <Loading label={t(lang, "filtersReading")} />
      ) : filters.length === 0 ? (
        failed ? null : <Empty>{t(lang, "filtersNone")}</Empty>
      ) : (
        <ul className="app-reports" aria-label={t(lang, "myFilters")}>
          {filters.map((filter) => (
            <li key={filter.id}>
              <button type="button" className="app-place" aria-label={t(lang, "applyFilter", { name: filter.name })} onClick={() => onApply(filter.view)}>
                {filter.name}
              </button>
              <button
                type="button"
                className="jc-button"
                aria-label={t(lang, "forgetFilter", { name: filter.name })}
                disabled={filter.id < 0}
                onClick={() => void forget(filter)}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}
