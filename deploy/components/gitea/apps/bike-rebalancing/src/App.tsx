import { useMemo } from "react";
import { AppShell } from "@joinedcontext/sdk";
import type { ShellPage } from "@joinedcontext/sdk";
import { AccessProvider, useAccess } from "./access";
import { History } from "./components/History";
import { LevelsCard } from "./components/LevelsCard";
import { LANGS, useLang } from "./i18n";
import type { Lang } from "./i18n";
import { Rebalance } from "./pages/Rebalance";
import { apiBase, plansApi } from "./plans";
import type { PlansApi } from "./plans";
import { t } from "./texts";
import { readParams, writeParams } from "./url";

const LANGUAGES = [
  { code: "fi", label: "Suomi" },
  { code: "en", label: "English" },
  { code: "sv", label: "Svenska" },
];

/**
 * Suomi, English or Svenska: the choice is written into the address (`?lang=`) and the page reloads in it,
 * so every text, number and date follows at once and a shared link keeps the language.
 */
export function chooseLanguage(next: string, current: Lang, reload: () => void = () => window.location.reload()): void {
  if (next === current || !LANGS.includes(next as Lang)) return;
  const params = readParams(window.location.search);
  params.set("lang", next);
  writeParams(params);
  reload();
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

function Shell({ api }: { api: PlansApi }) {
  const lang = useLang();
  const { me } = useAccess();
  const pages: ShellPage[] = [
    { id: "plan", label: t(lang, "page"), render: () => <Rebalance plans={api} /> },
    { id: "levels", label: t(lang, "levelsPage"), render: () => <LevelsCard lang={lang} /> },
    { id: "history", label: t(lang, "historyPage"), render: () => <History lang={lang} api={api} /> },
  ];
  return (
    <AppShell
      title={t(lang, "title")}
      pages={pages}
      languages={LANGUAGES}
      language={lang}
      onLanguage={(next) => chooseLanguage(next, lang)}
      userName={me?.name}
      actions={<RoleBadge lang={lang} />}
    />
  );
}

/** The operator's rebalancing plan in the SDK's shell (SDK-39): the stations to empty and fill, the van's route between them, the levels a steward sets and the history of changes. */
export default function App({ plans }: { plans?: PlansApi } = {}) {
  const api = useMemo(() => plans ?? plansApi(apiBase()), [plans]);
  return (
    <AccessProvider api={api}>
      <Shell api={api} />
    </AccessProvider>
  );
}
