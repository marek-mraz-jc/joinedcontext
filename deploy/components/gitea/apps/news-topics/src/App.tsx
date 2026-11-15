import { useMemo, useState } from "react";
import { AppShell, useClient } from "@joinedcontext/sdk";
import type { ShellPage } from "@joinedcontext/sdk";
import { AccessProvider, useAccess } from "./access";
import { Changes } from "./components/Changes";
import { getLanguage, t } from "./i18n";
import type { Lang } from "./i18n";
import { Topics } from "./pages/Topics";
import { useServer } from "./server";
import type { Server } from "./server";

const LANGUAGES = [
  { code: "fi", label: "Suomi" },
  { code: "en", label: "English" },
  { code: "sv", label: "Svenska" },
];

/** Who reads the page, in the shell's header: their role, or that they are signed out. */
export function RoleBadge({ lang }: { lang: Lang }) {
  const { me, steward } = useAccess();
  if (me === undefined) return null;
  const role = me === null ? t("signedOut", lang) : t(steward ? "roleSteward" : "roleViewer", lang);
  return (
    <span className="app-role" aria-label={`${t("roleLabel", lang)}: ${role}`}>
      {role}
    </span>
  );
}

function Shell({ server }: { server: Server }) {
  const { me } = useAccess();
  const [lang, setLang] = useState<Lang>(getLanguage);
  // The shell offers only LANGUAGES' codes; the address keeps the choice for the page's own reads.
  const switchTo = (code: string) => {
    const params = new URLSearchParams(window.location.search);
    params.set("lang", code);
    try {
      window.history.replaceState(null, "", `${window.location.pathname}?${params}${window.location.hash}`);
    } catch {
      // A sandboxed preview may refuse; the page still switches.
    }
    setLang(code as Lang);
  };
  document.documentElement.lang = lang;
  const pages: ShellPage[] = useMemo(
    () => [
      { id: "topics", label: t("topics", lang), render: () => <Topics key={lang} /> },
      { id: "changes", label: t("changesPage", lang), render: () => <Changes lang={lang} api={server} /> },
    ],
    [lang, server],
  );
  return (
    <AppShell
      title={t("appTitle", lang)}
      pages={pages}
      initial="topics"
      languages={LANGUAGES}
      language={lang}
      onLanguage={switchTo}
      userName={me?.name}
      actions={<RoleBadge lang={lang} />}
    />
  );
}

/**
 * Helsinki news topics in the SDK's shell (SDK-39): topic shares, weekly trends, and the articles
 * of a topic, each of which opens in the shell's entity panel (SDK-40). A reader follows topics;
 * a steward names and merges them (T-3828).
 */
export default function App() {
  const server = useServer(useClient().config.appName);
  return (
    <AccessProvider api={server}>
      <Shell server={server} />
    </AccessProvider>
  );
}
