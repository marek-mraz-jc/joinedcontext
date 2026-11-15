import { useMemo, useReducer } from "react";
import { AppShell } from "@joinedcontext/sdk";
import type { ShellPage } from "@joinedcontext/sdk";
import { AccessProvider, useAccess } from "./access";
import { History } from "./components/History";
import { MyDays } from "./components/MyDays";
import { LANGS, useLang } from "./i18n";
import type { Lang } from "./i18n";
import { MyDay } from "./pages/MyDay";
import { apiBase, shareApi } from "./share";
import type { ShareApi } from "./share";
import { t } from "./texts";
import { readParams, writeParams } from "./url";

const LANGUAGES = [
  { code: "fi", label: "Suomi" },
  { code: "en", label: "English" },
  { code: "sv", label: "Svenska" },
];

/**
 * Suomi, English or Svenska: written into the address (`?lang=`), which every text, number and date
 * reads, so the page redraws in it at once and a shared link keeps the language.
 */
export function chooseLanguage(next: string, current: Lang, redraw: () => void): void {
  if (next === current || !LANGS.includes(next as Lang)) return;
  const params = readParams(window.location.search);
  params.set("lang", next);
  writeParams(params);
  redraw();
}

/** Who reads the page, in the shell's header: their role, or that they are signed out. */
export function RoleBadge({ lang }: { lang: Lang }) {
  const { me, steward } = useAccess();
  if (me === undefined) return null;
  const role = me === null ? t(lang, "signedOut") : t(lang, steward ? "roleSteward" : "roleViewer");
  return (
    <span className="app-role" aria-label={`${t(lang, "roleLabel")}: ${role}`}>
      {role}
    </span>
  );
}

function Shell({ api }: { api: ShareApi }) {
  const lang = useLang();
  const { me } = useAccess();
  const [, redraw] = useReducer((count: number) => count + 1, 0);
  const pages: ShellPage[] = [
    { id: "day", label: t(lang, "page"), render: () => <MyDay shares={api} /> },
    { id: "days", label: t(lang, "daysPage"), render: () => <MyDays lang={lang} api={api} /> },
    { id: "history", label: t(lang, "historyPage"), render: () => <History lang={lang} api={api} /> },
  ];
  return (
    <AppShell
      title={t(lang, "title")}
      pages={pages}
      languages={LANGUAGES}
      language={lang}
      onLanguage={(next) => chooseLanguage(next, lang, redraw)}
      userName={me?.name}
      actions={<RoleBadge lang={lang} />}
    />
  );
}

/**
 * A visitor's day of Helsinki's events in the SDK's shell (SDK-39): what to see, in which order,
 * and the walk between; an event opens in the shell's entity panel (SDK-40). A steward features
 * events for everyone; a signed-in visitor keeps their shared days and sees their own history.
 */
export default function App({ shares }: { shares?: ShareApi } = {}) {
  const api = useMemo(() => shares ?? shareApi(apiBase()), [shares]);
  return (
    <AccessProvider api={api}>
      <Shell api={api} />
    </AccessProvider>
  );
}
