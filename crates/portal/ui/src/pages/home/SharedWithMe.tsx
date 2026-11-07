import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import { localized } from "../../api/manifest";
import { useBranding } from "../../branding";
import { Card, PageFailed, safeHref } from "../../components/ui";
import { appAddress, inLanguage } from "../apps/AppOpenPage";

/**
 * Shared with me (AP-174): the published Apps of other projects shared with the whole
 * organization that the person may open. The server lists only those, so a title here is always
 * one the person opens; each opens on its own host, in the Portal's language.
 */
export function SharedWithMe(): JSX.Element {
  const { t, i18n } = useTranslation();
  const { appsOrigin } = useBranding();
  const shared = useQuery({
    queryKey: ["apps", "shared"],
    queryFn: async () => unwrap(await api.GET("/api/v1/shared-apps")),
  });
  // A list, or nothing to draw: a page never breaks on an answer of another shape.
  const apps = Array.isArray(shared.data) ? shared.data : [];

  return (
    <section aria-labelledby="home-shared-heading" className="space-y-3">
      <div className="space-y-1">
        <h2 id="home-shared-heading" className="text-title font-semibold text-fg">
          {t("home.shared.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("home.shared.lead")}</p>
      </div>
      {shared.isError ? (
        <PageFailed error={shared.error} onRetry={() => void shared.refetch()} />
      ) : shared.isPending ? (
        <p className="text-body text-fg-muted" role="status">
          {t("home.shared.loading")}
        </p>
      ) : apps.length === 0 ? (
        <p className="rounded-lg border border-dashed border-border p-4 text-body text-fg-muted">{t("home.shared.empty")}</p>
      ) : (
        <ul className="grid gap-3 sm:grid-cols-2">
          {apps.map((app) => {
            const title = localized(app.title as Record<string, string> | string | null | undefined, i18n.language, "") || app.name;
            const description = localized(app.description as Record<string, string> | string | null | undefined, i18n.language, "");
            const address = inLanguage(new URL(appAddress(app.name, appsOrigin), window.location.origin).toString(), i18n.language);
            return (
              <Card as="li" padding="md" key={`${app.project}/${app.name}`} className="hover:bg-surface-subtle">
                <a href={safeHref(address)} target="_blank" rel="noopener noreferrer" className="focus-ring block h-full rounded">
                  <span className="block text-body font-semibold text-fg">{title}</span>
                  {description ? <span className="block text-caption text-fg-muted line-clamp-2">{description}</span> : null}
                  <span className="mt-1 block text-caption text-fg-muted">
                    {t("home.shared.from", { project: app.project })}
                  </span>
                  <span className="sr-only">{t("home.shared.newTab")}</span>
                </a>
              </Card>
            );
          })}
        </ul>
      )}
    </section>
  );
}
