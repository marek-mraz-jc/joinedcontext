import { useState } from "react";
import { AppShell, useClient } from "@joinedcontext/sdk";
import type { ShellPage } from "@joinedcontext/sdk";
import { AccessProvider, useAccess } from "./access";
import { Changes } from "./components/Changes";
import { langOf, t } from "./i18n";
import type { Lang } from "./i18n";
import { Compare } from "./pages/Compare";
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
  const [lang, setLang] = useState<Lang>(() =>
    langOf(window.location.search, navigator.languages ?? [navigator.language]),
  );

  const switchTo = (next: string) => {
    if (next !== "fi" && next !== "en" && next !== "sv") return;
    setLang(next);
    document.documentElement.lang = next;
    document.title = t(next, "title");
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
    {
      id: "compare",
      label: t(lang, "page"),
      render: () => <Compare key={scheme} lang={lang} />,
    },
    { id: "changes", label: t(lang, "changesPage"), render: () => <Changes lang={lang} api={server} /> },
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
 * Compare districts side by side, in Finnish, English or Swedish, with URL hash persistence, in
 * the SDK's shell (SDK-39). A reader keeps and shares comparisons; a steward pins one (T-3828).
 */
export default function App(): React.JSX.Element {
  const client = useClient();
  const server = useServer(client.config.appName);
  return (
    <AccessProvider api={server}>
      <Shell server={server} />
    </AccessProvider>
  );
}
