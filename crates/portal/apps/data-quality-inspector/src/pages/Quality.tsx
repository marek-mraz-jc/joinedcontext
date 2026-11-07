import { useEffect, useMemo, useState } from "react";
import type { Row } from "@joinedcontext/sdk";
import { Card, Empty, Loading, Page, Problem, ProblemError, currentTokens, useClient, useEntitySelection, useSchema } from "@joinedcontext/sdk";
import { Trend } from "../components/Trend";
import { FindingAction } from "../components/FindingAction";
import { useAccess } from "../access";
import { problemText } from "../problems";
import { findingKey, useResolutions } from "../resolutions";
import { useServer } from "../server";
import { ChartCard } from "../components/ChartCard";
import type { Lang } from "../i18n";
import { number, t } from "../i18n";
import { useInspect } from "../inspect";
import type { TypeQuality } from "../quality";
import {
  formatAge,
  formatRule,
  localId,
  percent,
  readHash,
  resolveTypes,
  sortTypesByQuality,
  toInput,
  writeHash,
} from "../quality";

interface TypeData {
  rows: Row[];
  error: ProblemError | Error | null;
}

/** Builds the ECharts options for completeness by type bar chart. */
export function completenessBarOption(types: TypeQuality[], _lang: Lang): Record<string, unknown> | null {
  if (types.length === 0) return null;
  const tokens = currentTokens();
  return {
    grid: { left: 48, right: 16, top: 24, bottom: 8, containLabel: true },
    tooltip: {
      trigger: "axis",
      formatter: (params: unknown) => {
        const p = Array.isArray(params) ? params[0] : params;
        const val = (p as { value?: unknown })?.value;
        const name = (p as { name?: unknown })?.name;
        return `${name}: ${val}%`;
      },
    },
    xAxis: {
      type: "category",
      data: types.map((t) => t.type),
      axisLabel: { interval: 0, rotate: types.length > 5 || window.innerWidth < 640 ? 35 : 0 },
    },
    yAxis: {
      type: "value",
      min: 0,
      max: 100,
      axisLabel: { formatter: "{value} %" },
    },
    series: [
      {
        type: "bar",
        data: types.map((t) => Math.round(t.completeness * 100)),
        itemStyle: { color: tokens.color.accent },
      },
    ],
  };
}

/**
 * Helsinki data quality overview and type detail inspection screen. A failing entity opens in the
 * SDK's entity panel (SDK-40) by its id. A steward resolves a finding or corrects its attribute
 * through the App's Endpoint (T-3827); everyone sees what was resolved.
 */
export function Quality({ lang }: { lang: Lang }): React.JSX.Element {
  const client = useClient();
  const server = useServer(client.config.appName);
  const { select } = useEntitySelection();
  const { steward } = useAccess();
  const resolutions = useResolutions(server);
  const [acting, setActing] = useState<{ entity: string; attribute: string; id: string } | null>(null);
  const [said, setSaid] = useState<{ text: string; problem: boolean } | null>(null);
  const { schema, error: schemaError } = useSchema();

  const [selectedType, setSelectedType] = useState<string | null>(() => {
    const hash = typeof window !== "undefined" ? window.location.hash : "";
    return readHash(hash).type;
  });

  const [dataMap, setDataMap] = useState<Map<string, TypeData>>(new Map());
  const [loadedCount, setLoadedCount] = useState<number>(0);
  const [loading, setLoading] = useState<boolean>(true);
  const [nonce, setNonce] = useState<number>(0);

  // The served types are the schema's; until it answers nothing is read, and only a schema that
  // cannot be read falls back to the types dev holds (validity then not checked).
  const types = useMemo(
    () => (schema ? resolveTypes(schema) : schemaError ? resolveTypes(null) : []),
    [schema, schemaError],
  );

  const reload = () => {
    setNonce((n) => n + 1);
  };

  useEffect(() => {
    const onHashChange = () => {
      const next = readHash(window.location.hash);
      setSelectedType(next.type);
    };
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  const selectType = (tName: string | null) => {
    setSelectedType(tName);
    try {
      window.history.replaceState(null, "", writeHash(tName));
    } catch {
      // sandboxed preview may refuse
    }
  };

  // Load entities for all served types, updating progress count per completed type
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setLoadedCount(0);
    const currentMap = new Map<string, TypeData>();

    let completed = 0;
    const load = async (tName: string) => {
      try {
        const rows = await client.entities.all(tName);
        if (cancelled) return;
        currentMap.set(tName, { rows, error: null });
      } catch (err) {
        if (cancelled) return;
        const error =
          err instanceof ProblemError
            ? err
            : new ProblemError(0, { title: err instanceof Error ? err.message : String(err) });
        currentMap.set(tName, { rows: [], error });
      } finally {
        if (!cancelled) {
          completed += 1;
          setLoadedCount(completed);
        }
      }
    };
    // Two types at a time: every type at once is a burst of paged reads that ran into the
    // gateway's rate limit on dev (429 on three of thirteen types, T-3578).
    const queue = [...types];
    const reader = async () => {
      for (let next = queue.shift(); next !== undefined && !cancelled; next = queue.shift()) await load(next);
    };
    const promises = [reader(), reader()];

    void Promise.all(promises).then(() => {
      if (!cancelled) {
        setDataMap(new Map(currentMap));
        setLoading(false);
      }
    });

    return () => {
      cancelled = true;
    };
  }, [client, types, nonce]);

  const failedTypes = useMemo(() => {
    const list: string[] = [];
    for (const [tName, data] of dataMap.entries()) {
      if (data.error) list.push(tName);
    }
    return list;
  }, [dataMap]);

  const inspectInput = useMemo(() => {
    if (loading || dataMap.size === 0) return null;
    const successful = new Map<string, Row[]>();
    for (const [tName, data] of dataMap.entries()) {
      if (!data.error) {
        successful.set(tName, data.rows);
      }
    }
    if (successful.size === 0) return null;
    const now = Math.floor(Date.now() / 1000);
    return toInput(now, schema, successful);
  }, [loading, dataMap, schema]);

  const { output, running, error: inspectError } = useInspect(inspectInput);

  const qualityList = useMemo(() => {
    if (!output) return [];
    return sortTypesByQuality(output.types);
  }, [output]);

  const selectedQuality = useMemo(() => {
    if (!selectedType || !output) return null;
    return output.types.find((t) => t.type === selectedType) ?? null;
  }, [selectedType, output]);

  const chartOption = useMemo(() => completenessBarOption(qualityList, lang), [qualityList, lang]);

  const allTypesFailed = !loading && types.length > 0 && failedTypes.length === types.length;

  return (
    <Page label={t(lang, "title")}>
      {inspectError ? (
        <Problem error={new Error(t(lang, "inspectFailed", { reason: inspectError.message }))} />
      ) : null}

      {schemaError && <Problem error={new Error(t(lang, "schemaUnreadable"))} />}

      {allTypesFailed ? (
        <Problem
          error={new ProblemError(0, { title: t(lang, "allTypesFailed") })}
          onRetry={reload}
        />
      ) : null}

      <div className="app-type-bar">
        <ul className="app-type-list" role="list" aria-label={t(lang, "typesList")}>
          {types.map((tName) => (
            <li key={tName}>
              <button
                type="button"
                className="app-type-btn"
                aria-pressed={selectedType === tName}
                onClick={() => selectType(selectedType === tName ? null : tName)}
              >
                {tName}
              </button>
            </li>
          ))}
        </ul>
      </div>

      {(types.length === 0 && !schemaError) || (loading && dataMap.size < types.length) ? (
        <Loading
          label={
            loadedCount > 0
              ? t(lang, "loadingProgress", { done: loadedCount, total: types.length })
              : t(lang, "loading")
          }
        />
      ) : running && !output ? (
        <Loading label={t(lang, "analysing")} />
      ) : selectedType === null ? (
        <>
          <div className="jc-tiles">
            {qualityList.map((q) => {
              const medianAge = q.freshness
                ? formatAge(q.freshness.medianSeconds, lang)
                : t(lang, "noTimestamp");
              return (
                <div
                  key={q.type}
                  role="button"
                  tabIndex={0}
                  className="jc-tile app-tile-clickable"
                  onClick={() => selectType(q.type)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      selectType(q.type);
                    }
                  }}
                  aria-label={`${q.type}: ${q.entities} ${t(lang, "entities").toLowerCase()}`}
                >
                  <span className="jc-tile-label">{q.type}</span>
                  <span className="jc-tile-value">{number(lang, q.entities)}</span>
                  <div className="app-tile-stats">
                    <div className="app-tile-stat">
                      <span className="app-tile-stat-k">{t(lang, "completeness")}</span>
                      <span className="app-tile-stat-v">{percent(q.completeness, lang)}</span>
                    </div>
                    <div className="app-tile-stat">
                      <span className="app-tile-stat-k">{t(lang, "valid")}</span>
                      <span className="app-tile-stat-v">
                        {q.valid !== null ? percent(q.valid, lang) : t(lang, "noSchema")}
                      </span>
                    </div>
                    <div className="app-tile-stat">
                      <span className="app-tile-stat-k">{t(lang, "medianAge")}</span>
                      <span className="app-tile-stat-v">{medianAge}</span>
                    </div>
                  </div>
                </div>
              );
            })}
            {failedTypes.map((tName) => (
              <div key={tName} className="jc-tile app-tile-error">
                <span className="jc-tile-label">{tName}</span>
                <span className="app-tile-err-msg">{t(lang, "typeReadError")}</span>
                <button type="button" className="app-tile-retry" onClick={reload}>
                  {t(lang, "retry")}
                </button>
              </div>
            ))}
          </div>

          <ChartCard
            title={t(lang, "chartTitle")}
            option={chartOption}
            height={300}
            empty={t(lang, "chartEmpty")}
            onSelect={(name) => selectType(name)}
          />
        </>
      ) : (
        <>
          <div className="app-detail-header">
            <button
              type="button"
              className="jc-button app-back-btn"
              onClick={() => selectType(null)}
            >
              ← {t(lang, "allTypes")}
            </button>
          </div>

          {failedTypes.includes(selectedType) ? (
            <Problem
              error={dataMap.get(selectedType)?.error ?? new Error(t(lang, "typeReadError"))}
              onRetry={reload}
            />
          ) : selectedQuality ? (
            <>
              <div className="jc-tiles">
                <div className="jc-tile">
                  <span className="jc-tile-label">{t(lang, "entities")}</span>
                  <span className="jc-tile-value">{number(lang, selectedQuality.entities)}</span>
                </div>
                <div className="jc-tile">
                  <span className="jc-tile-label">{t(lang, "completeness")}</span>
                  <span className="jc-tile-value">{percent(selectedQuality.completeness, lang)}</span>
                </div>
                <div className="jc-tile">
                  <span className="jc-tile-label">{t(lang, "valid")}</span>
                  <span className="jc-tile-value">
                    {selectedQuality.valid !== null
                      ? percent(selectedQuality.valid, lang)
                      : t(lang, "noSchema")}
                  </span>
                </div>
              </div>

              <Card title={t(lang, "freshness")}>
                {selectedQuality.freshness ? (
                  <dl className="app-freshness-dl">
                    <dt>{t(lang, "freshnessField")}</dt>
                    <dd>
                      <code>{selectedQuality.freshness.field}</code>
                    </dd>
                    <dt>{t(lang, "medianAge")}</dt>
                    <dd>{formatAge(selectedQuality.freshness.medianSeconds, lang)}</dd>
                    <dt>{t(lang, "oldest")}</dt>
                    <dd>{formatAge(selectedQuality.freshness.maxSeconds, lang)}</dd>
                    <dt>{t(lang, "olderThan24hTitle")}</dt>
                    <dd>{percent(selectedQuality.freshness.olderThanDay, lang)}</dd>
                  </dl>
                ) : (
                  <p className="app-note">{t(lang, "noTimestamp")}</p>
                )}
              </Card>

              {selectedQuality.valid === null && (
                <p className="app-note">{t(lang, "noSchemaDetails")}</p>
              )}

              <Card title={t(lang, "attributesTable", { type: selectedType })}>
                <div className="jc-table-wrap">
                  <table
                    className="jc-table"
                    aria-label={t(lang, "attributesTable", { type: selectedType })}
                  >
                    <thead>
                      <tr>
                        <th scope="col">{t(lang, "attribute")}</th>
                        <th scope="col" className="jc-num">
                          {t(lang, "completeness")}
                        </th>
                        <th scope="col">{t(lang, "required")}</th>
                        <th scope="col" className="jc-num">
                          {t(lang, "notChecked")}
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {selectedQuality.attributes.map((attr) => (
                        <tr key={attr.name}>
                          <th scope="row">
                            <code>{attr.name}</code>
                          </th>
                          <td className="jc-num" data-label={t(lang, "completeness")}>
                            {percent(attr.completeness, lang)}
                          </td>
                          <td data-label={t(lang, "required")}>
                            {attr.required ? t(lang, "yes") : t(lang, "no")}
                          </td>
                          <td className="jc-num" data-label={t(lang, "notChecked")}>
                            {attr.notChecked > 0 ? number(lang, attr.notChecked) : "—"}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </Card>

              <Card title={t(lang, "failingTable")}>
                {selectedQuality.findingsTotal === 0 ? (
                  <Empty>{t(lang, "allValid")}</Empty>
                ) : (
                  <>
                    <div className="jc-table-wrap">
                      <table className="jc-table" aria-label={t(lang, "failingTable")}>
                        <thead>
                          <tr>
                            <th scope="col">{t(lang, "id")}</th>
                            <th scope="col">{t(lang, "attribute")}</th>
                            <th scope="col">{t(lang, "failedRule")}</th>
                            <th scope="col">{t(lang, "state")}</th>
                          </tr>
                        </thead>
                        <tbody>
                          {selectedQuality.findings.slice(0, 200).map((f, idx) => (
                            <tr key={`${f.entity}-${f.attribute}-${f.rule}-${idx}`}>
                              <td data-label={t(lang, "id")}>
                                <button type="button" className="app-open" title={f.entity} onClick={() => select({ id: f.entity, type: selectedType })}>
                                  <code>{localId(f.entity)}</code>
                                </button>
                              </td>
                              <td data-label={t(lang, "attribute")}>
                                <code>{f.attribute}</code>
                              </td>
                              <td data-label={t(lang, "failedRule")}>{formatRule(f.rule, f.detail, lang)}</td>
                              <td data-label={t(lang, "state")}>
                                {(() => {
                                  const done = resolutions.byFinding.get(findingKey(f.entity, f.attribute));
                                  const id = localId(f.entity);
                                  const closed = steward ? undefined : t(lang, "stewardOnly");
                                  if (done)
                                    return (
                                      <span className="app-state">
                                        <span>{t(lang, done.corrected ? "correctedBy" : "resolvedBy", { by: done.resolverName, note: done.note })}</span>
                                        <button
                                          type="button"
                                          className="jc-button"
                                          disabled={!steward}
                                          title={closed}
                                          aria-label={`${t(lang, "reopen")}: ${f.attribute} ${id}`}
                                          onClick={() => {
                                            setSaid(null);
                                            resolutions.reopen(done.id).then(
                                              () => setSaid({ text: t(lang, "reopened"), problem: false }),
                                              (error: unknown) => setSaid({ text: problemText(lang, error), problem: true }),
                                            );
                                          }}
                                        >
                                          {t(lang, "reopen")}
                                        </button>
                                      </span>
                                    );
                                  return (
                                    <span className="app-state">
                                      <span>{t(lang, "open")}</span>
                                      <button
                                        type="button"
                                        className="jc-button"
                                        disabled={!steward}
                                        title={closed}
                                        aria-label={`${t(lang, "resolve")}: ${f.attribute} ${id}`}
                                        onClick={() => {
                                          setSaid(null);
                                          setActing({ entity: f.entity, attribute: f.attribute, id });
                                        }}
                                      >
                                        {t(lang, "resolve")}
                                      </button>
                                    </span>
                                  );
                                })()}
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                    <p role="status" aria-live="polite" className={said?.problem ? "app-error" : "app-note"}>
                      {said?.text ?? (resolutions.failed ? problemText(lang, resolutions.failed) : steward ? "" : t(lang, "stewardOnly"))}
                    </p>
                    {acting && steward && (
                      <FindingAction
                        key={findingKey(acting.entity, acting.attribute)}
                        lang={lang}
                        resolutions={resolutions}
                        finding={acting}
                        onDone={(text) => {
                          setActing(null);
                          setSaid({ text, problem: false });
                        }}
                        onCancel={() => setActing(null)}
                      />
                    )}
                    {selectedQuality.findingsTotal > 200 && (
                      <p className="app-failing-count">
                        {t(lang, "failingCapped", {
                          shown: 200,
                          total: selectedQuality.findingsTotal,
                        })}
                      </p>
                    )}
                  </>
                )}
              </Card>
            </>
          ) : (
            <Empty>{t(lang, "noEntities")}</Empty>
          )}
        </>
      )}
      <Trend lang={lang} server={server} />
    </Page>
  );
}
