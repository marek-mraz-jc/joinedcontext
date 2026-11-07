/**
 * The accessibility statement (T-5108): what a Slovak public body must publish under Act
 * 95/2019 and Decree 78/2020 (WCAG 2.1 AA), in the model of EU decision 2018/1523: status,
 * what is not accessible and its alternative, how it was checked, when, whom to write and where
 * to turn next. Public: a visitor reads it before signing in. The catalogue host serves the same
 * statement from its own theme (deployment charts/ckan, PAGES["accessibility"]).
 */
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useBranding } from "../../branding";
import { PageHeader, safeHref } from "../../components/ui";

/** When the statement was written and last checked against the walker's axe results. */
export const STATEMENT_DATES = { prepared: "2026-10-21", reviewed: "2026-10-21" } as const;

/** An address a mail link can carry: one `@`, a dotted domain, nothing that breaks out of the URL. */
const MAILBOX = /^[^\s@<>"/:?#]+@[^\s@<>"/:?#]+\.[^\s@<>"/:?#]+$/;

const GAPS = ["maps", "charts", "screenReader", "thirdParty"] as const;

export function AccessibilityPage(): JSX.Element {
  const { t, i18n } = useTranslation();
  const { organisation, instanceName, contactEmail } = useBranding();
  const operator = organisation || instanceName;
  const hasMailbox = MAILBOX.test(contactEmail);
  const day = (iso: string) =>
    new Intl.DateTimeFormat(i18n.language, { dateStyle: "long", timeZone: "UTC" }).format(new Date(`${iso}T00:00:00Z`));
  const section = (key: string, body: ReactNode) => (
    <section aria-labelledby={`a11y-${key}`} className="space-y-2">
      <h2 id={`a11y-${key}`} className="text-title font-semibold text-fg">
        {t(`accessibility.${key}.heading`)}
      </h2>
      {body}
    </section>
  );
  return (
    <article className="mx-auto max-w-3xl space-y-6 px-4 py-6 text-body text-fg">
      <PageHeader title={t("accessibility.title")} description={t("accessibility.lead", { organisation: operator })} />
      {section("law", <p>{t("accessibility.law.body", { host: window.location.host })}</p>)}
      {section("status", <p>{t("accessibility.status.body")}</p>)}
      {section(
        "gaps",
        <ul className="list-disc space-y-1 pl-6">
          {GAPS.map((gap) => (
            <li key={gap}>{t(`accessibility.gaps.${gap}`)}</li>
          ))}
        </ul>,
      )}
      {section("method", <p>{t("accessibility.method.body")}</p>)}
      {section(
        "dates",
        <p>
          {t("accessibility.dates.body", {
            prepared: day(STATEMENT_DATES.prepared),
            reviewed: day(STATEMENT_DATES.reviewed),
          })}
        </p>,
      )}
      {section(
        "contact",
        <>
          <p>{t("accessibility.contact.body")}</p>
          {hasMailbox ? (
            <p>
              {t("accessibility.contact.write")}{" "}
              <a className="text-primary-soft-fg underline hover:no-underline" href={safeHref(`mailto:${contactEmail}`)}>
                {contactEmail}
              </a>
            </p>
          ) : (
            <p>{t("accessibility.contact.noEmail", { organisation: operator })}</p>
          )}
        </>,
      )}
      {section("enforcement", <p>{t("accessibility.enforcement.body")}</p>)}
    </article>
  );
}
