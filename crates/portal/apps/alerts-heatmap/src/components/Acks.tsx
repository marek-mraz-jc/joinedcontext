import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { Card } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import { moment, t } from "../i18n";
import type { Lang } from "../i18n";
import type { Ack, Server } from "../server";

// The server and fetch fail with an Error each.
const reason = (error: unknown) => (error as Error).message;

/** An alert as the card lists it: its id and its name in words. */
export interface Listed {
  id: string;
  name: string;
}

/**
 * The newest alerts and who acknowledged them (T-3828): everyone reads the acknowledgements, a
 * steward gives one with a comment or takes it back. Shown at once, taken back when refused.
 */
export function Acks({ lang, server, alerts }: { lang: Lang; server: Server; alerts: Listed[] }) {
  const { me, steward } = useAccess();
  const [acks, setAcks] = useState<Ack[]>([]);
  const [failed, setFailed] = useState<string | null>(null);
  const [comments, setComments] = useState<Record<string, string>>({});
  const [said, setSaid] = useState<{ text: string; problem: boolean } | null>(null);

  useEffect(() => {
    let gone = false;
    server.acks().then(
      (answer) => !gone && setAcks(answer),
      (error: unknown) => !gone && setFailed(reason(error)),
    );
    return () => {
      gone = true;
    };
  }, [server]);

  const acknowledge = async (event: FormEvent, alert: Listed) => {
    event.preventDefault();
    const comment = (comments[alert.id] ?? "").trim();
    if (!comment || comment.length > 300) {
      setSaid({ text: t(lang, "commentNeeded"), problem: true });
      return;
    }
    const draft: Ack = { id: -Date.now(), alert: alert.id, comment, ownerName: me?.name ?? "", at: new Date().toISOString() };
    setAcks((current) => [draft, ...current]);
    setComments((current) => ({ ...current, [alert.id]: "" }));
    setSaid(null);
    try {
      const kept = await server.acknowledge(alert.id, comment);
      setAcks((current) => current.map((ack) => (ack.id === draft.id ? kept : ack)));
      setSaid({ text: t(lang, "acknowledged", { name: alert.name }), problem: false });
    } catch (error) {
      setAcks((current) => current.filter((ack) => ack.id !== draft.id));
      setComments((current) => ({ ...current, [alert.id]: comment }));
      setSaid({ text: t(lang, "refused", { reason: reason(error) }), problem: true });
    }
  };

  const takeBack = async (ack: Ack, alert: Listed) => {
    setAcks((current) => current.filter((other) => other.id !== ack.id));
    setSaid(null);
    try {
      await server.unacknowledge(ack.id);
      setSaid({ text: t(lang, "unacknowledged", { name: alert.name }), problem: false });
    } catch (error) {
      setAcks((current) => [ack, ...current]);
      setSaid({ text: t(lang, "refused", { reason: reason(error) }), problem: true });
    }
  };

  // Why the controls are off, as their tooltip; nothing when the reader may use them.
  const why = steward ? undefined : t(lang, me ? "ackStewardOnly" : "ackSignIn");
  return (
    <Card title={t(lang, "acks")}>
      <p className="app-note">{t(lang, "acksNote")}</p>
      {said && (
        <p className={said.problem ? "jc-problem" : "app-note"} role={said.problem ? "alert" : "status"}>
          {said.text}
        </p>
      )}
      {failed && (
        <p className="jc-problem" role="alert">
          {t(lang, "acksFailed", { reason: failed })}
        </p>
      )}
      <ul className="app-acks" aria-label={t(lang, "acks")}>
        {alerts.map((alert) => {
          const ack = acks.find((one) => one.alert === alert.id);
          return (
            <li key={alert.id}>
              <strong>{alert.name}</strong>
              <span className="app-note">
                {ack
                  ? t(lang, "ackedBy", { comment: ack.comment, who: ack.ownerName, at: moment(lang, Date.parse(ack.at)) })
                  : t(lang, "notAcked")}
              </span>
              {ack ? (
                <button
                  type="button"
                  className="jc-button"
                  disabled={!steward || ack.id < 0}
                  title={why}
                  aria-label={t(lang, "unacknowledge", { name: alert.name })}
                  onClick={() => void takeBack(ack, alert)}
                >
                  {t(lang, "unackButton")}
                </button>
              ) : (
                <form className="app-save" onSubmit={(event) => void acknowledge(event, alert)}>
                  <label>
                    <span>{t(lang, "comment", { name: alert.name })}</span>
                    <input
                      value={comments[alert.id] ?? ""}
                      maxLength={300}
                      disabled={!steward}
                      title={why}
                      onChange={(event) => setComments((current) => ({ ...current, [alert.id]: event.target.value }))}
                    />
                  </label>
                  <button type="submit" className="jc-button" disabled={!steward} title={why} aria-label={t(lang, "acknowledge", { name: alert.name })}>
                    {t(lang, "ackButton")}
                  </button>
                </form>
              )}
            </li>
          );
        })}
      </ul>
    </Card>
  );
}
