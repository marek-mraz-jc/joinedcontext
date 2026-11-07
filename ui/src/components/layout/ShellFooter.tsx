import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { safeHref } from "../ui";

/**
 * The signed-in page's footer: the accessibility statement a public body links from every page
 * (T-5108; it left the account menu in T-6344), the glossary, and the organisation's name and
 * contact address when the branding sets them.
 */
export function ShellFooter({ organisation, contactEmail }: { organisation?: string; contactEmail?: string }): JSX.Element {
  const { t } = useTranslation();
  const link = "focus-ring touch-target rounded-sm underline hover:text-fg hover:no-underline";
  return (
    <footer className="border-t border-border bg-surface px-4 py-3 text-caption text-fg-muted sm:px-gutter">
      <nav aria-label={t("accessibility.footer")} className="flex flex-wrap items-center gap-x-4 gap-y-1">
        {organisation ? <span>{organisation}</span> : null}
        {contactEmail ? (
          <a href={safeHref(`mailto:${contactEmail}`)} className={link}>
            {contactEmail}
          </a>
        ) : null}
        <a href="/accessibility" className={link}>
          {t("accessibility.link")}
        </a>
        <a href="/glossary" className={link}>
          {t("glossary.page.title")}
        </a>
      </nav>
    </footer>
  );
}
