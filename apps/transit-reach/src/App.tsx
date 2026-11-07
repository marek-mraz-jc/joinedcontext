import { useMemo, useState } from "react";
import { AppShell } from "@joinedcontext/sdk";
import type { ShellPage } from "@joinedcontext/sdk";
import { AccessProvider, useAccess } from "./access";
import { History } from "./components/History";
import { langOf, LANGS, t } from "./i18n";
import type { Lang } from "./i18n";
import { Origins } from "./pages/Origins";
import { Reach } from "./pages/Reach";
import { apiBase, reachApi } from "./server";
import type { ReachApi } from "./server";
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

function Shell({ api }: { api: ReachApi }) {
  const { me } = useAccess();
  const [lang, setLang] = useState<Lang>(() => langOf(window.location.search, navigator.languages ?? [navigator.language]));
  const switchTo = (next: string) => {
    if (next === lang || !LANGS.includes(next as Lang)) return;
    setLang(next as Lang);
    try {
      const params = new URLSearchParams(window.location.search);
      params.set("lang", next);
      window.history.replaceState(null, "", `${window.location.pathname}?${params}${window.location.hash}`);
    } catch {
      // A sandboxed preview may refuse; the page still switches.
    }
  };
  document.documentElement.lang = lang;
  document.title = t(lang, "title");
  // A new scheme redraws the page, so the map and its legend take their colours again.
  const scheme = useScheme();
  const pages: ShellPage[] = [
    { id: "reach", label: t(lang, "page"), render: () => <Reach key={scheme} lang={lang} server={api} /> },
    { id: "origins", label: t(lang, "originsPage"), render: () => <Origins lang={lang} api={api} /> },
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
 * How far one gets by transit in Helsinki, in Finnish, English or Swedish, the language kept in the
 * address; a signed-in visitor keeps origins to compare, a steward publishes a shared set (T-3826).
 */
export default function App({ server }: { server?: ReachApi } = {}) {
  const api = useMemo(() => server ?? reachApi(apiBase()), [server]);
  return (
    <AccessProvider api={api}>
      <Shell api={api} />
    </AccessProvider>
  );
}
