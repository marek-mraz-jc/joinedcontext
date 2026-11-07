import { useEffect, useMemo, useState } from "react";
import { Card, Empty, Loading, Page, Problem, Split, currentTokens, format, useClient, useEntities, useEntitySelection } from "@joinedcontext/sdk";
import type { Row } from "@joinedcontext/sdk";
import { ChartCard } from "../components/ChartCard";
import { FollowedCard, useFollows } from "../components/Follows";
import { KeptWeeks } from "../components/KeptWeeks";
import { NamesCard, useNames } from "../components/Names";
import { useAccess } from "../access";
import { formatDate, formatPercent, getLanguage, t } from "../i18n";
import type { Lang } from "../i18n";
import { mergeNamed, topTerm, useTopics } from "../topics";
import type { Topic, WeekShare } from "../topics";
import { useServer } from "../server";

const ENTITY_TYPE = "Article";

interface HashState {
  weeks: string;
  k: number;
  topic: number | null;
  q: string;
}

function readHash(): HashState {
  const hash = window.location.hash;
  // No `?` in the address: no parameter, so every default.
  const params = new URLSearchParams(hash.includes("?") ? hash.slice(hash.indexOf("?") + 1) : "");
  const rawWeeks = params.get("weeks");
  const weeks = rawWeeks === "4" || rawWeeks === "8" || rawWeeks === "12" || rawWeeks === "all" ? rawWeeks : "all";
  const rawK = parseInt(params.get("k") ?? "5", 10);
  const k = Number.isInteger(rawK) && rawK >= 3 && rawK <= 8 ? rawK : 5;
  const rawTopic = params.get("topic");
  const topic = rawTopic !== null && !Number.isNaN(parseInt(rawTopic, 10)) ? parseInt(rawTopic, 10) : null;
  const q = params.get("q") ?? "";
  return { weeks, k, topic, q };
}

function writeHash(state: HashState) {
  const params = new URLSearchParams();
  if (state.weeks !== "all") {
    params.set("weeks", state.weeks);
  }
  if (state.k !== 5) {
    params.set("k", String(state.k));
  }
  if (state.topic !== null) {
    params.set("topic", String(state.topic));
  }
  if (state.q.trim()) {
    params.set("q", state.q.trim());
  }
  const qs = params.toString();
  const nextHash = qs ? `#topics?${qs}` : "#topics";
  if (window.location.hash !== nextHash) {
    try {
      window.history.replaceState(null, "", nextHash);
    } catch {
      // Sandboxed previews may refuse replaceState
    }
  }
}

function filterRows(rows: Row[], weeks: string, query: string): Row[] {
  let result = rows;
  // `weeks` is "4", "8", "12" or "all": readHash takes nothing else.
  if (weeks !== "all") {
    const cutoff = Date.now() - Number(weeks) * 7 * 24 * 60 * 60 * 1000;
    result = result.filter((row) => {
      const time = typeof row.datePublished === "string" ? new Date(row.datePublished).getTime() : Number.NaN;
      return !Number.isNaN(time) && time >= cutoff;
    });
  }

  if (query.trim()) {
    const q = query.trim().toLowerCase();
    result = result.filter((row) => {
      const title = format(row.name).toLowerCase();
      return title.includes(q);
    });
  }

  return result;
}

function sortArticles(articles: Row[]): Row[] {
  return [...articles].sort((a, b) => {
    const da = typeof a.datePublished === "string" ? new Date(a.datePublished).getTime() : NaN;
    const db = typeof b.datePublished === "string" ? new Date(b.datePublished).getTime() : NaN;
    const aValid = !Number.isNaN(da);
    const bValid = !Number.isNaN(db);
    if (aValid && bValid) return db - da;
    if (aValid && !bValid) return -1;
    if (!aValid && bValid) return 1;
    return 0;
  });
}

/** A topic as the page names it: a steward's name, else its number. */
export function topicLabel(topic: Topic, lang: Lang, nameOf: (term: string) => string | undefined = () => undefined): string {
  return nameOf(topTerm(topic)) ?? `${t("topicN", lang)} ${topic.id + 1}`;
}

export function buildWeeklyChartOption(
  weeks: WeekShare[],
  topics: Topic[],
  lang: Lang,
  labelOf: (topic: Topic) => string = (topic) => topicLabel(topic, lang),
): Record<string, unknown> | null {
  if (weeks.length === 0 || topics.length === 0) return null;
  const palette = currentTokens().chart.palette;
  const categories = weeks.map((w) => w.week);

  const series = topics.map((topic, index) => {
    const color = palette[index % palette.length];
    const topKeywords = topic.keywords.slice(0, 3).map((k) => k.term).join(", ");
    const name = `${labelOf(topic)}${topKeywords ? `: ${topKeywords}` : ""}`;
    const data = weeks.map((w) => {
      const share = w.shares[topic.id] ?? 0;
      return Math.round(share * 1000) / 10;
    });

    return {
      name,
      type: "bar",
      stack: "total",
      itemStyle: { color },
      emphasis: { focus: "series" },
      data,
    };
  });

  return {
    tooltip: {
      trigger: "axis",
      axisPointer: { type: "shadow" },
      valueFormatter: (value: unknown) => (typeof value === "number" ? `${value.toFixed(1)}%` : `${value}`),
    },
    legend: {
      bottom: 0,
      type: "scroll",
    },
    grid: {
      top: 24,
      left: 12,
      right: 24,
      bottom: 40,
      containLabel: true,
    },
    xAxis: {
      type: "category",
      data: categories,
    },
    yAxis: {
      type: "value",
      min: 0,
      max: 100,
      axisLabel: { formatter: "{value}%" },
    },
    series,
  };
}

export function Topics(): React.JSX.Element {
  const lang = getLanguage();
  const [hashState, setHashState] = useState<HashState>(readHash);
  const [showTable, setShowTable] = useState(false);
  const { select } = useEntitySelection();

  const { rows, loading: entitiesLoading, error: entitiesError, reload } = useEntities(ENTITY_TYPE);
  const server = useServer(useClient().config.appName);
  const { me } = useAccess();
  const names = useNames(server);
  const follows = useFollows(server);

  useEffect(() => {
    const onHashChange = () => {
      setHashState(readHash());
    };
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  const filteredRows = useMemo(
    () => filterRows(rows, hashState.weeks, hashState.q),
    [rows, hashState.weeks, hashState.q],
  );

  const { status: topicsStatus, result: fitted, error: topicsError } = useTopics(filteredRows, hashState.k);
  // Topics a steward gave one name show as one (T-3828).
  const { nameOf } = names;
  const result = useMemo(() => (fitted ? mergeNamed(fitted, nameOf) : null), [fitted, nameOf]);
  const labelOf = (topic: Topic) => topicLabel(topic, lang, nameOf);

  const selectedTopic = useMemo(() => {
    if (!result || result.topics.length === 0) return null;
    if (hashState.topic !== null) {
      const match = result.topics.find((tp) => tp.id === hashState.topic);
      if (match) return match;
    }
    return result.topics[0];
  }, [result, hashState.topic]);

  const selectedArticles = useMemo(() => {
    if (!selectedTopic) return [];
    const articleMap = new Map(rows.map((r) => [r.id, r]));
    const matched = selectedTopic.articles
      .map((id) => articleMap.get(id))
      .filter((r): r is Row => r !== undefined);
    return sortArticles(matched);
  }, [selectedTopic, rows]);

  const chartOption = useMemo(
    () => (result ? buildWeeklyChartOption(result.weeks, result.topics, lang, (topic) => topicLabel(topic, lang, nameOf)) : null),
    [result, lang, nameOf],
  );

  const handlePeriodChange = (nextWeeks: string) => {
    const next = { ...hashState, weeks: nextWeeks };
    setHashState(next);
    writeHash(next);
  };

  const handleKChange = (nextK: number) => {
    const next = { ...hashState, k: nextK };
    setHashState(next);
    writeHash(next);
  };

  const handleSearchChange = (nextQ: string) => {
    const next = { ...hashState, q: nextQ };
    setHashState(next);
    writeHash(next);
  };

  const handleTopicClick = (topicId: number) => {
    const next = { ...hashState, topic: topicId };
    setHashState(next);
    writeHash(next);
  };

  const combinedError = entitiesError || topicsError;

  return (
    <Page label={t("pageTitle", lang)}>
      {combinedError && (
        <Problem error={combinedError} onRetry={entitiesError ? reload : undefined} />
      )}

      <form className="app-filters" role="search" aria-label={t("filters", lang)} onSubmit={(e) => e.preventDefault()}>
        <label>
          <span>{t("period", lang)}</span>
          <select value={hashState.weeks} onChange={(e) => handlePeriodChange(e.target.value)}>
            <option value="4">{t("weeks4", lang)}</option>
            <option value="8">{t("weeks8", lang)}</option>
            <option value="12">{t("weeks12", lang)}</option>
            <option value="all">{t("all", lang)}</option>
          </select>
        </label>
        <label>
          <span>{t("topicsCount", lang)}</span>
          <select value={hashState.k} onChange={(e) => handleKChange(Number(e.target.value))}>
            {[3, 4, 5, 6, 7, 8].map((num) => (
              <option key={num} value={num}>
                {num}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>{t("search", lang)}</span>
          <input
            type="search"
            value={hashState.q}
            onChange={(e) => handleSearchChange(e.target.value)}
            placeholder={t("searchPlaceholder", lang)}
          />
        </label>
      </form>

      {entitiesLoading && rows.length === 0 ? (
        <Loading label={t("reading", lang)} />
      ) : filteredRows.length === 0 ? (
        <Empty>{t("empty", lang)}</Empty>
      ) : topicsStatus === "loading" && !result ? (
        <Loading label={t("loading", lang)} />
      ) : result && result.topics.length > 0 ? (
        <>
          <Split ratio="1:1">
            <Card title={t("topicsTitle", lang)}>
              <ul className="app-topics" aria-label={t("topicsTitle", lang)}>
                {result.topics.map((item) => {
                  const isSelected = selectedTopic?.id === item.id;
                  const term = topTerm(item);
                  const following = follows.follows.some((follow) => follow.term === term);
                  return (
                    <li key={item.id}>
                      <button
                        type="button"
                        className="app-topic"
                        onClick={() => handleTopicClick(item.id)}
                        aria-pressed={isSelected}
                        aria-label={[`${t("topicN", lang)} ${item.id + 1}`, item.keywords.map((kw) => kw.term).join(", ")].filter(Boolean).join(": ")}
                        aria-describedby={`topic-${item.id}-name topic-${item.id}-share topic-${item.id}-count`}
                      >
                        <span className="app-topic-head">
                          <strong id={`topic-${item.id}-name`}>{labelOf(item)}</strong>
                          <span className="app-topic-share" id={`topic-${item.id}-share`}>{formatPercent(item.share, lang)}</span>
                        </span>
                        <span className="app-topic-keywords">
                          <span className="app-sr-only">{t("keywords", lang)}: </span>
                          {item.keywords.map((kw) => (
                            <span key={kw.term} className="app-keyword">
                              {kw.term}
                            </span>
                          ))}
                        </span>
                        <span className="app-topic-count" id={`topic-${item.id}-count`}>
                          {item.articles.length} {t("articlesCount", lang)}
                        </span>
                      </button>
                      {term && (
                        <button
                          type="button"
                          className="jc-button app-follow"
                          aria-pressed={following}
                          aria-label={t("follow", lang, { name: term })}
                          disabled={!me}
                          title={me ? undefined : t("followSignIn", lang)}
                          onClick={() => void follows.toggle(term, lang)}
                        >
                          {t(following ? "followingShort" : "followShort", lang)}
                        </button>
                      )}
                    </li>
                  );
                })}
              </ul>
            </Card>

            <Card title={t("chartTitle", lang)}>
              <ChartCard
                title={t("chartTitle", lang)}
                option={chartOption}
                loading={topicsStatus === "loading"}
                error={topicsError}
                empty={result.weeks.length === 0 ? t("empty", lang) : undefined}
              />
              <div className="app-table-toggle">
                <button
                  type="button"
                  className="jc-button"
                  onClick={() => setShowTable((prev) => !prev)}
                  aria-expanded={showTable}
                >
                  {showTable ? t("hideTable", lang) : t("showTable", lang)}
                </button>
                {/* Hidden from the eye until asked for, never from a screen reader: the chart's text. */}
                <div className={showTable ? "jc-table-wrap" : "app-sr-only"}>
                  <table className="jc-table" aria-label={t("tableAria", lang)}>
                    <thead>
                      <tr>
                        <th scope="col">{t("week", lang)}</th>
                        {result.topics.map((tp) => (
                          <th key={tp.id} scope="col">
                            {labelOf(tp)}
                          </th>
                        ))}
                      </tr>
                    </thead>
                    <tbody>
                      {result.weeks.map((w) => (
                        <tr key={w.week}>
                          <td>{w.week}</td>
                          {result.topics.map((tp) => (
                            <td key={tp.id} className="jc-num">
                              {formatPercent(w.shares[tp.id] ?? 0, lang)}
                            </td>
                          ))}
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            </Card>
          </Split>

          <Card
            title={`${t("articlesTitle", lang)}: ${selectedTopic ? labelOf(selectedTopic) : ""} (${selectedArticles.length})`}
          >
            {selectedArticles.length === 0 ? (
              <Empty>{t("empty", lang)}</Empty>
            ) : (
              <ul className="app-events" aria-label={t("articlesTitle", lang)}>
                {selectedArticles.map((article) => {
                  const title = format(article.name) || article.id;
                  const desc = format(article.description);
                  // Only a web address is a link: a feed entry holding anything else gets none.
                  const url = /^https:\/\/\S+$/.test(format(article.url)) ? format(article.url) : "";
                  const pub = format(article.datePublished);
                  return (
                    <li key={article.id} className="app-event">
                      <div className="app-event-body">
                        <h3>
                          {/* SDK-40: the article opens in the shell's entity panel, linked to the Portal. */}
                          <button type="button" className="app-open" onClick={() => select({ id: article.id, type: ENTITY_TYPE })}>
                            {title}
                          </button>
                        </h3>
                        <p className="app-event-when">
                          {pub ? (
                            <time dateTime={pub}>{formatDate(pub, lang)}</time>
                          ) : (
                            <span>{t("noDate", lang)}</span>
                          )}
                        </p>
                        {desc && <p>{desc}</p>}
                        {url && (
                          <p className="app-event-tags">
                            <a href={url} target="_blank" rel="noopener noreferrer" aria-label={`${t("readOriginal", lang)}: ${title}`}>
                              {t("readOriginal", lang)}
                            </a>
                          </p>
                        )}
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}
          </Card>
        </>
      ) : (
        <Empty>{t("empty", lang)}</Empty>
      )}
      <Split ratio="1:1">
        <FollowedCard lang={lang} state={follows} present={(term) => (result?.topics ?? []).some((topic) => topic.keywords.some((kw) => kw.term === term))} />
        <NamesCard lang={lang} state={names} selected={selectedTopic} topics={result?.topics ?? []} labelOf={labelOf} />
      </Split>
      <KeptWeeks lang={lang} server={server} />
    </Page>
  );
}

