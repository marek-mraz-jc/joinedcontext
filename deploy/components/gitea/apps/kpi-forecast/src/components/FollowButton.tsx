import { useState } from "react";
import { useAccess } from "../access";
import { useFollows } from "../follows";
import { t } from "../i18n";
import type { Lang } from "../i18n";
import { problemText } from "./Earlier";

/** Follow the indicator, or stop following it; signed out, the button says why it is closed. */
export function FollowButton({ lang, kpi, name }: { lang: Lang; kpi: string; name: string }) {
  const { me } = useAccess();
  const follows = useFollows();
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const followed = follows.list?.find((f) => f.kpi === kpi);
  const act = async () => {
    setBusy(true);
    setMessage(null);
    try {
      if (followed) await follows.unfollow(followed.id);
      else await follows.follow(kpi);
      setMessage({ text: t(lang, followed ? "unfollowed" : "followed", { name }), error: false });
    } catch (error) {
      setMessage({ text: problemText(lang, error), error: true });
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <button
        type="button"
        className="jc-button app-open"
        aria-pressed={Boolean(followed)}
        disabled={!me || busy || follows.list === null}
        title={me ? undefined : t(lang, "followSignIn")}
        onClick={() => void act()}
      >
        {t(lang, followed ? "unfollow" : "follow")}
      </button>
      <span role="status" aria-live="polite" className={message?.error ? "app-error" : "app-note"}>
        {message?.text ?? ""}
      </span>
    </>
  );
}
