import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";
import { Card, Empty } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { t } from "../i18n";
import type { Lang } from "../i18n";
import type { Server, TopicName } from "../server";
import { topTerm } from "../topics";
import type { Topic } from "../topics";

// The server and fetch fail with an Error each.
const reason = (error: unknown) => (error as Error).message;

export interface Names {
  names: TopicName[];
  failed: string | null;
  /** A steward's name for the topic of `term`, if it has one. */
  nameOf(term: string): string | undefined;
  /** Names the topic of `term`; shown at once, the old name back when refused. Throws the refusal. */
  give(term: string, name: string, owner: string): Promise<void>;
  /** Takes a name back; shown at once, back when refused. Throws the refusal. */
  takeBack(entry: TopicName): Promise<void>;
}

/** The topic names everyone reads, read once. */
export function useNames(server: Server): Names {
  const [names, setNames] = useState<TopicName[]>([]);
  const [failed, setFailed] = useState<string | null>(null);

  useEffect(() => {
    let gone = false;
    server.names().then(
      (answer) => !gone && setNames(answer),
      (error: unknown) => !gone && setFailed(reason(error)),
    );
    return () => {
      gone = true;
    };
  }, [server]);

  const byTerm = useMemo(() => new Map(names.map((entry) => [entry.term, entry.name])), [names]);
  const nameOf = useCallback((term: string) => byTerm.get(term), [byTerm]);

  const give = async (term: string, name: string, owner: string) => {
    const before = names.find((entry) => entry.term === term);
    const draft: TopicName = { id: before?.id ?? -Date.now(), term, name, ownerName: owner };
    setNames((current) => [...current.filter((entry) => entry.term !== term), draft]);
    try {
      const made = await server.name(term, name);
      setNames((current) => current.map((entry) => (entry.term === term ? made : entry)));
    } catch (error) {
      setNames((current) => [...current.filter((entry) => entry.term !== term), ...(before ? [before] : [])]);
      throw error;
    }
  };

  const takeBack = async (entry: TopicName) => {
    setNames((current) => current.filter((other) => other.id !== entry.id));
    try {
      await server.unname(entry.id);
    } catch (error) {
      setNames((current) => [...current, entry]);
      throw error;
    }
  };

  return { names, failed, nameOf, give, takeBack };
}

/**
 * A steward names the selected topic, or merges it with another by giving both one name: the
 * other's name, else its keywords. Everyone reads the names given; a viewer's controls stay
 * closed with the reason.
 */
export function NamesCard({
  lang,
  state,
  selected,
  topics,
  labelOf,
}: {
  lang: Lang;
  state: Names;
  selected: Topic | null;
  topics: Topic[];
  labelOf: (topic: Topic) => string;
}) {
  const { me, steward } = useAccess();
  const [draft, setDraft] = useState("");
  const [into, setInto] = useState("");
  const [said, setSaid] = useState<{ text: string; problem: boolean } | null>(null);
  const why = steward ? undefined : t(me ? "stewardOnly" : "stewardSignIn", lang);
  const owner = me?.name ?? "";
  const others = topics.filter((topic) => topic.id !== selected?.id);

  const run = async (work: () => Promise<void>, done: string) => {
    setSaid(null);
    try {
      await work();
      setSaid({ text: done, problem: false });
    } catch (error) {
      setSaid({ text: t("refused", lang, { reason: reason(error) }), problem: true });
    }
  };

  const name = (event: FormEvent) => {
    event.preventDefault();
    const named = draft.trim();
    if (!selected || !named || named.length > 80) {
      setSaid({ text: t("nameNeeded", lang), problem: true });
      return;
    }
    setDraft("");
    void run(() => state.give(topTerm(selected), named, owner), t("named", lang, { name: named }));
  };

  const merge = (event: FormEvent) => {
    event.preventDefault();
    const target = others.find((topic) => String(topic.id) === into) ?? others[0];
    if (!selected || !target) return;
    const shared =
      state.nameOf(topTerm(target)) ??
      target.keywords
        .slice(0, 3)
        .map((keyword) => keyword.term)
        .join(", ");
    void run(async () => {
      if (!state.nameOf(topTerm(target))) await state.give(topTerm(target), shared, owner);
      await state.give(topTerm(selected), shared, owner);
    }, t("merged", lang, { name: shared }));
  };

  return (
    <Card title={t("names", lang)}>
      <p className="app-note">{t("namesNote", lang)}</p>
      {said && (
        <p className={said.problem ? "jc-problem" : "app-note"} role={said.problem ? "alert" : "status"}>
          {said.text}
        </p>
      )}
      {state.failed && (
        <p className="jc-problem" role="alert">
          {t("namesFailed", lang, { reason: state.failed })}
        </p>
      )}
      {selected && (
        <>
          <form className="app-filters" onSubmit={name}>
            <label>
              <span>{t("nameOf", lang)}</span>
              <input value={draft} maxLength={80} disabled={!steward} title={why} onChange={(event) => setDraft(event.target.value)} />
            </label>
            <button type="submit" className="jc-button" disabled={!steward} title={why}>
              {t("nameTopic", lang)}
            </button>
          </form>
          {others.length > 0 && (
            <form className="app-filters" onSubmit={merge}>
              <label>
                <span>{t("mergeWith", lang)}</span>
                <select value={into} disabled={!steward} title={why} onChange={(event) => setInto(event.target.value)}>
                  {others.map((topic) => (
                    <option key={topic.id} value={topic.id}>
                      {labelOf(topic)}
                    </option>
                  ))}
                </select>
              </label>
              <button type="submit" className="jc-button" disabled={!steward} title={why}>
                {t("merge", lang)}
              </button>
            </form>
          )}
        </>
      )}
      {state.names.length === 0 ? (
        state.failed ? null : <Empty>{t("namesNone", lang)}</Empty>
      ) : (
        <ul className="app-list" aria-label={t("names", lang)}>
          {state.names.map((entry) => (
            <li key={entry.term}>
              <span>{t("nameLine", lang, { term: entry.term, name: entry.name, who: entry.ownerName })}</span>
              <button
                type="button"
                className="jc-button"
                disabled={!steward || entry.id < 0}
                title={why}
                aria-label={t("unname", lang, { term: entry.term })}
                onClick={() => void run(() => state.takeBack(entry), t("unnamed", lang, { name: entry.name }))}
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
