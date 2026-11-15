import { useMemo, useState } from "react";
import { AppShell } from "@joinedcontext/sdk";
import type { ShellPage } from "@joinedcontext/sdk";
import { AccessProvider, useAccess } from "./access";
import { History } from "./components/History";
import { FollowsProvider } from "./follows";
import { langOf, t } from "./i18n";
import type { Lang } from "./i18n";
import { Follows } from "./pages/Follows";
import { Forecast } from "./pages/Forecast";
import { apiBase, kpiApi } from "./server";
import type { KpiApi } from "./server";
import { useScheme } from "./theme";

const LANGUAGES = [
  { code: "fi", label: "Suomi" },
  { code: "en", label: "English" },
  { code: "sv", label: "Svenska" },
];

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

function Shell({ api }: { api: KpiApi }) {
  const { me } = useAccess();
  const [lang, setLang] = useState<Lang>(() => langOf(window.location.search, navigator.languages ?? [navigator.language]));
  const switchTo = (next: string) => {
    const chosen: Lang = next === "fi" || next === "sv" ? next : "en";
    setLang(chosen);
    try {
      const params = new URLSearchParams(window.location.search);
      params.set("lang", chosen);
      window.history.replaceState(null, "", `${window.location.pathname}?${params}${window.location.hash}`);
    } catch {
      // A sandboxed preview may refuse; the page still switches.
    }
  };
  document.documentElement.lang = lang;
  document.title = t(lang, "title");
  // A new scheme redraws the page, so the chart takes its colours again.
  const scheme = useScheme();
  const pages: ShellPage[] = [
    { id: "kpis", label: t(lang, "page"), render: () => <Forecast key={scheme} lang={lang} server={api} /> },
    { id: "follows", label: t(lang, "followsPage"), render: () => <Follows lang={lang} /> },
    { id: "history", label: t(lang, "historyPage"), render: () => <History lang={lang} api={api} /> },
  ];
  return (
    <AppShell
      title={t(lang, "title")}
      pages={pages}
      languages={LANGUAGES}
      language={lang}
      onLanguage={switchTo}
      userName={me?.name}
      actions={<RoleBadge lang={lang} />}
    />
  );
}

/**
 * Helsinki's indicators, their trends and forecasts, in the SDK's shell (SDK-39): Finnish, English
 * or Swedish, the language kept in the address; an indicator opens in the SDK's entity panel
 * (SDK-40). A reader follows indicators; a steward publishes forecasts (T-3827).
 */
export default function App({ server }: { server?: KpiApi } = {}) {
  const api = useMemo(() => server ?? kpiApi(apiBase()), [server]);
  return (
    <AccessProvider api={api}>
      <FollowsProvider api={api}>
        <Shell api={api} />
      </FollowsProvider>
    </AccessProvider>
  );
}
