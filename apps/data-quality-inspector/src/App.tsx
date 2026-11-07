import { useState } from "react";
import { AppShell, useClient } from "@joinedcontext/sdk";
import type { ShellPage } from "@joinedcontext/sdk";
import { AccessProvider, useAccess } from "./access";
import { History } from "./components/History";
import { langOf, t } from "./i18n";
import type { Lang } from "./i18n";
import { Quality } from "./pages/Quality";
import { useServer } from "./server";
import type { Server } from "./server";
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

function Shell({ server }: { server: Server }) {
  const { me } = useAccess();
  const [lang, setLang] = useState<Lang>(() => langOf(window.location.search, navigator.languages ?? [navigator.language]));

  const switchTo = (next: string) => {
    if (next !== "fi" && next !== "en" && next !== "sv") return;
    setLang(next);
    try {
      const params = new URLSearchParams(window.location.search);
      params.set("lang", next);
      window.history.replaceState(null, "", `${window.location.pathname}?${params}${window.location.hash}`);
    } catch {
      // sandboxed preview may refuse
    }
  };

  document.documentElement.lang = lang;
  document.title = t(lang, "title");
  const scheme = useScheme();

  const pages: ShellPage[] = [
    { id: "quality", label: t(lang, "page"), render: () => <Quality key={scheme} lang={lang} /> },
    { id: "history", label: t(lang, "historyPage"), render: () => <History lang={lang} api={server} /> },
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
 * Helsinki data quality inspector application, light and dark, Finnish, English and Swedish, in
 * the SDK's shell (SDK-39). A steward resolves findings and corrects attributes (T-3827).
 */
export default function App(): React.JSX.Element {
  const server = useServer(useClient().config.appName);
  return (
    <AccessProvider api={server}>
      <Shell server={server} />
    </AccessProvider>
  );
}
