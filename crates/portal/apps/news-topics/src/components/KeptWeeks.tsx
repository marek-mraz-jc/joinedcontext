import { useEffect, useState } from "react";
import { Card, Empty } from "@joinedcontext/sdk";
import { formatPercent, t } from "../i18n";
import type { Lang } from "../i18n";
import type { Kept, KeptWeek, Server } from "../server";

const reason = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** A week's topics in words: the three strongest keywords of each, with its share. */
function topicsOf(week: KeptWeek, lang: Lang): string {
  return week.topics
    .map((topic) => `${topic.keywords.slice(0, 3).map((k) => k.term).join(", ")} (${formatPercent(topic.share, lang, 0)})`)
    .join("; ");
}

/**
 * The topics of every week the App's server has kept (T-3352), the newest first, each week's
 * articles downloadable as a file: the feed holds only its recent articles, the server keeps
 * each week's topics after the feed has dropped them.
 */
export function KeptWeeks({ lang, server }: { lang: Lang; server: Server }) {
  const [kept, setKept] = useState<Kept | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [download, setDownload] = useState<string | null>(null);

  useEffect(() => {
    let gone = false;
    server.weeks().then(
      (answer) => !gone && setKept(answer),
      (error: unknown) => !gone && setFailed(reason(error)),
    );
    return () => {
      gone = true;
    };
  }, [server]);

  const corpus = async (week: string) => {
    setDownload(null);
    try {
      window.open(await server.corpusUrl(week), "_blank", "noopener");
    } catch (error) {
      setDownload(`${t("corpusFailed", lang)} ${reason(error)}`);
    }
  };

  return (
    <Card title={t("kept", lang)}>
      <p className="app-note">{t("keptNote", lang)}</p>
      {download && (
        <p className="jc-problem" role="alert">
          {download}
        </p>
      )}
      {failed ? (
        <p className="jc-problem" role="alert">
          {t("keptFailed", lang)} {failed}
        </p>
      ) : !kept ? (
        <p className="app-note">{t("keptReading", lang)}</p>
      ) : kept.weeks.length === 0 ? (
        <Empty>{t("keptEmpty", lang)}</Empty>
      ) : (
        <>
          {kept.stale && <p className="app-note">{t("keptStale", lang)}</p>}
          {/* Scrolls sideways on a phone: a keyboard reaches it as a named region (WCAG 2.1.1). */}
          <div className="jc-table-wrap" tabIndex={0} role="region" aria-label={t("keptTable", lang)}>
            <table className="jc-table" aria-label={t("keptTable", lang)}>
              <thead>
                <tr>
                  <th scope="col">{t("week", lang)}</th>
                  <th scope="col">{t("keptArticles", lang)}</th>
                  <th scope="col">{t("keptTopics", lang)}</th>
                  <th scope="col">{t("corpus", lang)}</th>
                </tr>
              </thead>
              <tbody>
                {kept.weeks.map((week) => (
                  <tr key={week.week}>
                    <th scope="row">{week.week}</th>
                    <td className="jc-num">{week.articles}</td>
                    <td>{topicsOf(week, lang) || "–"}</td>
                    <td>
                      <button type="button" className="jc-button" aria-label={`${t("corpusOf", lang)} ${week.week}`} onClick={() => void corpus(week.week)}>
                        {t("corpus", lang)}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
    </Card>
  );
}
