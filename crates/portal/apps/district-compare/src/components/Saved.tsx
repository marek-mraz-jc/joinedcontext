import { useEffect, useState } from "react";
import { Card, Empty, Loading } from "@joinedcontext/sdk";
import { useAccess } from "../access";
import type { Measure } from "../districts";
import { measureTitle, t } from "../i18n";
import type { Lang } from "../i18n";
import type { Comparison, NewComparison, Server } from "../server";

const reason = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** The address that opens the comparison whose share token is `share` for another reader of the
 * project, in the same language. */
export function shareLink(share: string, href = window.location.href): string {
  const url = new URL(href);
  const lang = url.searchParams.get("lang");
  url.search = "";
  url.hash = "";
  url.searchParams.set("comparison", share);
  if (lang) url.searchParams.set("lang", lang);
  return url.toString();
}

/**
 * The reader's own comparisons and the one a steward pinned for everyone (T-3828). A reader keeps
 * the comparison on screen under a name, opens, updates or deletes their own, and shares one by its
 * link; a steward pins the one on screen, or takes the pin back. Each change shows at once and is
 * taken back when the server refuses.
 */
export function Saved({
  lang,
  server,
  codes,
  measure,
  names,
  pinned,
  onPinned,
  onOpen,
}: {
  lang: Lang;
  server: Server;
  codes: string[];
  measure: Measure;
  /** District names by code, to name a comparison's districts. */
  names: Map<string, string>;
  pinned: Comparison | null;
  onPinned: (pinned: Comparison | null) => void;
  onOpen: (comparison: NewComparison) => void;
}) {
  const { me, steward } = useAccess();
  const [list, setList] = useState<Comparison[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);

  useEffect(() => {
    if (me === undefined) return;
    if (me === null) return;
    let current = true;
    server.comparisons().then(
      (kept) => {
        if (!current) return;
        setList(kept);
        setLoaded(true);
      },
      (error: unknown) => {
        if (!current) return;
        setLoaded(true);
        setFailed(reason(error));
      },
    );
    return () => {
      current = false;
    };
  }, [server, me]);

  const now: NewComparison = { name: name.trim(), codes, measure };
  const line = (c: NewComparison) => t(lang, "comparisonLine", { measure: measureTitle(lang, c.measure), districts: c.codes.map((code) => names.get(code) ?? code).join(", ") });

  /** Runs a change shown at once; `undo` takes it back when the server refuses. */
  const run = async (write: () => Promise<string>, undo: () => void) => {
    setBusy(true);
    setProblem(null);
    setDone(null);
    try {
      setDone(await write());
    } catch (error) {
      undo();
      setProblem(t(lang, "refused", { reason: reason(error) }));
    } finally {
      setBusy(false);
    }
  };

  const checked = (): string | null => {
    if (codes.length === 0) return t(lang, "keepNeedsDistricts");
    if (!now.name || now.name.length > 80) return t(lang, "keepNeedsName");
    return null;
  };

  const keep = () => {
    const wrong = checked();
    if (wrong) {
      setDone(null);
      setProblem(wrong);
      return;
    }
    const draft: Comparison = { ...now, id: -Date.now(), ownerName: me?.name ?? "" };
    setList((kept) => [draft, ...kept]);
    void run(
      async () => {
        const made = await server.keep(now);
        setList((kept) => kept.map((c) => (c.id === draft.id ? made : c)));
        setName("");
        return t(lang, "kept", { name: made.name });
      },
      () => setList((kept) => kept.filter((c) => c.id !== draft.id)),
    );
  };

  const update = (old: Comparison) => {
    const next = { name: old.name, codes, measure };
    setList((kept) => kept.map((c) => (c.id === old.id ? { ...c, ...next } : c)));
    void run(
      async () => {
        await server.change(old.id, next);
        return t(lang, "updated", { name: old.name });
      },
      () => setList((kept) => kept.map((c) => (c.id === old.id ? old : c))),
    );
  };

  const forget = (old: Comparison) => {
    const before = list;
    setList(before.filter((c) => c.id !== old.id));
    void run(
      async () => {
        await server.forget(old.id);
        return t(lang, "forgotten", { name: old.name });
      },
      () => setList(before),
    );
  };

  const pin = () => {
    const wrong = checked();
    if (wrong) {
      setDone(null);
      setProblem(wrong);
      return;
    }
    const before = pinned;
    onPinned({ ...now, id: -1, ownerName: me?.name ?? "" });
    void run(
      async () => {
        const made = await server.pin(now);
        onPinned(made);
        setName("");
        return t(lang, "pinned", { name: made.name });
      },
      () => onPinned(before),
    );
  };

  const unpin = () => {
    const before = pinned;
    onPinned(null);
    void run(
      async () => {
        await server.unpin();
        // An older pin, when there is one, is the default again.
        onPinned(await server.pinned());
        return t(lang, "unpinned");
      },
      () => onPinned(before),
    );
  };

  const closed = me ? undefined : t(lang, "savedSignIn");
  const pinClosed = steward ? undefined : t(lang, "pinStewardOnly");

  return (
    <Card title={t(lang, "saved")}>
      <h3 className="app-subhead">{t(lang, "pinnedTitle")}</h3>
      <p className="app-note" data-testid="pinned">
        {pinned ? t(lang, "pinnedBy", { name: pinned.name, owner: pinned.ownerName }) : t(lang, "pinnedNone")}
      </p>
      {pinned && <p className="app-note">{line(pinned)}</p>}
      <div className="app-actions">
        {pinned && (
          <button type="button" className="jc-button" onClick={() => onOpen(pinned)}>
            {t(lang, "openPinned")}
          </button>
        )}
        <button type="button" className="jc-button" disabled={!steward || busy} title={pinClosed} onClick={pin}>
          {t(lang, "pin")}
        </button>
        <button type="button" className="jc-button" disabled={!steward || busy || !pinned} title={pinClosed} onClick={unpin}>
          {t(lang, "unpin")}
        </button>
      </div>

      <p className="app-note">{me === null ? t(lang, "savedSignIn") : t(lang, "savedNote")}</p>
      <form
        className="app-keep"
        aria-label={t(lang, "keep")}
        onSubmit={(event) => {
          event.preventDefault();
          keep();
        }}
      >
        <label>
          <span>{t(lang, "comparisonName")}</span>
          <input value={name} maxLength={80} disabled={!me || busy} title={closed} onChange={(event) => setName(event.target.value)} />
        </label>
        <button type="submit" className="jc-button" disabled={!me || busy} title={closed}>
          {t(lang, "keep")}
        </button>
      </form>
      {problem && (
        <p className="app-error" role="alert">
          {problem}
        </p>
      )}
      {done && (
        <p className="app-note" role="status">
          {done}
        </p>
      )}
      {failed && (
        <p className="app-error" role="alert">
          {t(lang, "savedFailed", { reason: failed })}
        </p>
      )}
      {me === null ? null : !loaded ? (
        <Loading label={t(lang, "savedReading")} />
      ) : list.length === 0 ? (
        failed ? null : <Empty>{t(lang, "savedNone")}</Empty>
      ) : (
        <ul className="app-saved" aria-label={t(lang, "saved")}>
          {list.map((c) => (
            <li key={c.id}>
              <span>
                <strong>{c.name}</strong> {line(c)}
              </span>
              {/* A comparison still being kept has nothing to act on yet. */}
              {c.id > 0 && (
              <span className="app-actions">
                {c.share && <a href={shareLink(c.share)}>{t(lang, "shareComparison", { name: c.name })}</a>}
                <button type="button" className="jc-button" onClick={() => onOpen(c)}>
                  {t(lang, "openComparison", { name: c.name })}
                </button>
                <button
                  type="button"
                  className="jc-button"
                  disabled={busy || codes.length === 0}
                  title={codes.length === 0 ? t(lang, "keepNeedsDistricts") : undefined}
                  onClick={() => update(c)}
                >
                  {t(lang, "updateComparison", { name: c.name })}
                </button>
                <button type="button" className="jc-button" disabled={busy} onClick={() => forget(c)}>
                  {t(lang, "forgetComparison", { name: c.name })}
                </button>
              </span>
              )}
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}
