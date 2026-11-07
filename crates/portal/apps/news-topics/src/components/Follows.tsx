import { useEffect, useState } from "react";
import { Card, Empty, Loading } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { t } from "../i18n";
import type { Lang } from "../i18n";
import type { Follow, Server } from "../server";

// The server and fetch fail with an Error each.
const reason = (error: unknown) => (error as Error).message;

export interface Follows {
  follows: Follow[];
  ready: boolean;
  failed: string | null;
  said: { text: string; problem: boolean } | null;
  /** Follows `term`, or stops following it; shown at once, taken back when refused. */
  toggle(term: string, lang: Lang): Promise<void>;
}

/** The signed-in reader's followed keywords, read once they are known. */
export function useFollows(server: Server): Follows {
  const { me } = useAccess();
  const [follows, setFollows] = useState<Follow[]>([]);
  const [ready, setReady] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const [said, setSaid] = useState<Follows["said"]>(null);

  useEffect(() => {
    if (!me) return;
    let gone = false;
    server
      .follows()
      .then(
        (answer) => !gone && setFollows(answer),
        (error: unknown) => !gone && setFailed(reason(error)),
      )
      .finally(() => setReady(true));
    return () => {
      gone = true;
    };
  }, [server, me]);

  const toggle = async (term: string, lang: Lang) => {
    const kept = follows.find((follow) => follow.term === term);
    setSaid(null);
    if (kept) {
      setFollows((current) => current.filter((follow) => follow.id !== kept.id));
      try {
        await server.unfollow(kept.id);
        setSaid({ text: t("stoppedFollowing", lang, { name: term }), problem: false });
      } catch (error) {
        setFollows((current) => [kept, ...current]);
        setSaid({ text: t("refused", lang, { reason: reason(error) }), problem: true });
      }
      return;
    }
    const draft: Follow = { id: -Date.now(), term };
    setFollows((current) => [draft, ...current]);
    try {
      const made = await server.follow(term);
      setFollows((current) => current.map((follow) => (follow.id === draft.id ? made : follow)));
      setSaid({ text: t("startedFollowing", lang, { name: term }), problem: false });
    } catch (error) {
      setFollows((current) => current.filter((follow) => follow.id !== draft.id));
      setSaid({ text: t("refused", lang, { reason: reason(error) }), problem: true });
    }
  };

  return { follows, ready, failed, said, toggle };
}

/** The followed topics, each said to be among the period's topics or not, and let go of here. */
export function FollowedCard({ lang, state, present }: { lang: Lang; state: Follows; present: (term: string) => boolean }) {
  const { me } = useAccess();
  return (
    <Card title={t("followed", lang)}>
      <p className="app-note">{t("followedNote", lang)}</p>
      {state.said && (
        <p className={state.said.problem ? "jc-problem" : "app-note"} role={state.said.problem ? "alert" : "status"}>
          {state.said.text}
        </p>
      )}
      {state.failed && (
        <p className="jc-problem" role="alert">
          {t("followedFailed", lang, { reason: state.failed })}
        </p>
      )}
      {me === null ? (
        <Empty>{t("followSignIn", lang)}</Empty>
      ) : !state.ready ? (
        <Loading label={t("reading", lang)} />
      ) : state.follows.length === 0 ? (
        state.failed ? null : <Empty>{t("followedNone", lang)}</Empty>
      ) : (
        <ul className="app-list" aria-label={t("followed", lang)}>
          {state.follows.map((follow) => (
            <li key={follow.id}>
              <strong>{follow.term}</strong>
              <span className="app-note">{t(present(follow.term) ? "followedNow" : "followedNotNow", lang)}</span>
              <button
                type="button"
                className="jc-button"
                disabled={follow.id < 0}
                aria-label={t("unfollow", lang, { name: follow.term })}
                onClick={() => void state.toggle(follow.term, lang)}
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
