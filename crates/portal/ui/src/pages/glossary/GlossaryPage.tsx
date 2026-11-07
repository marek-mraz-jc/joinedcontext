/**
 * The platform's words, each in one sentence with one example (T-3236, UI-45). Every `Term` on a
 * page links here, to its own entry, so a person who wants more than the hint reads the whole
 * definition without leaving the Portal. Public: the words are the same for everybody.
 */
import { useEffect } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { PageHeader, TERMS } from "../../components/ui";

export function GlossaryPage(): JSX.Element {
  const { t } = useTranslation();
  // A link to `#term-…` lands on its entry once the page has drawn it.
  useEffect(() => {
    const target = window.location.hash ? document.getElementById(window.location.hash.slice(1)) : null;
    target?.scrollIntoView({ block: "start" });
  }, []);
  return (
    <section className="mx-auto max-w-3xl space-y-6 px-4 py-6" aria-label={t("glossary.page.title")}>
      <PageHeader title={t("glossary.page.title")} description={t("glossary.page.lead")} />
      <dl className="space-y-5">
        {TERMS.map((name) => (
          <div key={name} id={`term-${name}`} className="scroll-mt-20 space-y-1">
            <dt className="text-body font-semibold text-fg">{t(`glossary.${name}.term`)}</dt>
            <dd className="text-body text-fg">{t(`glossary.${name}.definition`)}</dd>
            <dd className="text-caption text-fg-muted">{t("glossary.page.example", { text: t(`glossary.${name}.example`) })}</dd>
          </div>
        ))}
      </dl>
    </section>
  );
}
