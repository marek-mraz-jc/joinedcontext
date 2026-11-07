/**
 * "Use this data" (T-3254): the view as curl, Python and JavaScript a person copies and runs as
 * written, against the endpoint they read through and with the filter they set, and how a program
 * gets its token when the endpoint is not public. The token itself is never in a snippet.
 */
import { useState } from "react";
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import type { EntityQuery } from "../../components/entities/filters";
import { CopyUrlButton, endpointUrl } from "../../components/endpoints/links";
import { CodeBlock, Button, Dialog } from "../../components/ui";
import { SNIPPET_LANGUAGES, TOKEN_VARIABLE, snippet, viewPath } from "./snippets";

export function UseThisData({
  project,
  slug,
  query,
  open,
}: {
  project: string;
  slug: string;
  /** The view as the export reads it: the type, the columns and both filters joined. */
  query: EntityQuery;
  /** The endpoint answers anyone (`audience: public`): no token is needed. */
  open: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const [shown, setShown] = useState(false);
  const url = endpointUrl(slug, viewPath(query));
  return (
    <>
      <Button size="sm" variant="secondary" onClick={() => setShown(true)}>
        {t("explore.useData.open")}
      </Button>
      <Dialog
        open={shown}
        onOpenChange={setShown}
        title={t("explore.useData.title")}
        description={t("explore.useData.lead", { type: query.type ?? "" })}
        closeLabel={t("app.close")}
      >
        <div className="space-y-4">
          {open ? (
            <p className="text-body text-fg-muted">{t("explore.useData.public")}</p>
          ) : (
            <section aria-labelledby="use-data-token" className="space-y-1">
              <h3 id="use-data-token" className="text-body font-semibold text-fg">
                {t("explore.useData.tokenTitle")}
              </h3>
              <p className="text-body text-fg-muted">{t("explore.useData.token", { variable: TOKEN_VARIABLE })}</p>
              <Link
                to="/projects/$project/settings/$tab"
                params={{ project, tab: "service-accounts" }}
                className="text-body text-primary-soft-fg underline"
              >
                {t("explore.useData.tokenLink")}
              </Link>
            </section>
          )}
          {SNIPPET_LANGUAGES.map((language) => {
            const code = snippet(language, url, open);
            return (
              <section key={language} aria-labelledby={`use-data-${language}`} className="space-y-1">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <h3 id={`use-data-${language}`} className="text-body font-semibold text-fg">
                    {t(`explore.useData.language.${language}`)}
                  </h3>
                  <CopyUrlButton url={code} label={t("explore.useData.copy", { language: t(`explore.useData.language.${language}`) })} />
                </div>
                <CodeBlock
                  label={t(`explore.useData.language.${language}`)}
                  className="max-h-64 rounded-md bg-surface-subtle p-2 font-mono text-caption"
                  data-snippet={language}
                >
                  {code}
                </CodeBlock>
              </section>
            );
          })}
        </div>
      </Dialog>
    </>
  );
}
