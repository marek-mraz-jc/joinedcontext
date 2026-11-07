import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { Badge } from "../../components/ui";
import type { BadgeTone } from "../../components/ui/Badge";

export type AppCheck = components["schemas"]["AppCheck"];

const TONE: Record<AppCheck["state"], BadgeTone> = {
  green: "success",
  red: "danger",
  amber: "warning",
};

/** The probe's last verdict on each App of `project`, by name (AP-136); refreshed every minute. */
export function useAppChecks(project: string): Map<string, AppCheck> {
  const checks = useQuery({
    queryKey: ["projects", project, "app-checks"],
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/app-checks", { params: { path: { project } } })),
    refetchInterval: 60_000,
  });
  return new Map((checks.data?.checks ?? []).map((check) => [check.name, check]));
}

/**
 * Whether the App worked the last time the probe opened it (AP-136): green, red with the reason,
 * amber when the probe has not run lately. The words carry the state, the colour only repeats
 * it; nothing is drawn before the first check.
 */
export function AppCheckChip({ check }: { check: AppCheck | undefined }): JSX.Element | null {
  const { t, i18n } = useTranslation();
  if (!check) return null;
  const when = new Intl.DateTimeFormat(i18n.resolvedLanguage ?? i18n.language ?? "en", {
    dateStyle: "short",
    timeStyle: "short",
  }).format(new Date(check.at));
  return (
    <span className="inline-flex max-w-full flex-col items-center gap-0.5">
      <Badge tone={TONE[check.state]}>{t(`apps.check.${check.state}`)}</Badge>
      <span className="text-caption text-fg-muted">
        {check.reason ? t("apps.check.lineWithReason", { when, reason: check.reason }) : t("apps.check.line", { when })}
      </span>
    </span>
  );
}
