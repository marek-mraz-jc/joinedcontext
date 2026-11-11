import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import {
  Badge,
  Card,
  CardHeader,
  PageFailed,
  PageLoading,
  EmptyState,
} from "../../components/ui";
import { AdminGrid } from "../../components/grid/AdminGrid";
import type { DataColumn } from "../../components/grid/AdminGrid";
import { bytesText } from "../knowledge/knowledge";
import { shortSha, useRepositoryRead } from "./api";
import type { Repository, RepositorySettings } from "./api";
import { when } from "./CommitsView";

type Rule = RepositorySettings["protection"][number];
type Package = RepositorySettings["packages"][number];

/** The settings are already read: nothing is loading or failing inside the grids. */
const LOADED = { isPending: false, isError: false, error: null, refetch: () => undefined };

/**
 * What the repository's administrators read: branch protection, webhooks (their address only,
 * the secret is never read back), who has access, and the packages and images published from it.
 * Changing them is a change to the project's configuration, proposed and approved in the Portal.
 */
export function SettingsView({ project, repository }: { project: string; repository: Repository }): JSX.Element {
  const { t, i18n } = useTranslation();
  const settings = useRepositoryRead<RepositorySettings>(project, "jc_repository_settings", { repository: repository.name });
  if (settings.isError) return <PageFailed error={settings.error} onRetry={() => void settings.refetch()} />;
  if (settings.isPending) return <PageLoading label={t("repositories.loading")} />;
  const data = settings.data;
  const language = i18n.resolvedLanguage ?? "en";
  const yes = (value: boolean | undefined) => (value ? t("repositories.settings.yes") : t("repositories.settings.no"));
  const ruleColumns: DataColumn<Rule>[] = [
    {
      key: "pattern",
      label: t("repositories.settings.pattern"),
      text: (rule) => rule.pattern ?? "",
      cell: (rule) => <span className="font-mono">{rule.pattern}</span>,
    },
    { key: "push", label: t("repositories.settings.push"), kind: "boolean", text: (rule) => yes(rule.pushAllowed) },
    {
      key: "forcePush",
      label: t("repositories.settings.forcePush"),
      kind: "boolean",
      text: (rule) => yes(rule.forcePushAllowed),
    },
    {
      key: "approvals",
      label: t("repositories.settings.approvals"),
      kind: "number",
      text: (rule) => String(rule.requiredApprovals ?? 0),
    },
  ];
  const packageColumns: DataColumn<Package>[] = [
    {
      key: "version",
      label: t("repositories.settings.version"),
      text: (item) => `${item.name} ${item.version}${item.signed ? ` ${t("repositories.settings.signed")}` : ""}`,
      cell: (item) => (
        <span className="font-mono">
          {item.name} {item.version}
          {item.signed ? (
            <Badge tone="success" className="ml-1">
              {t("repositories.settings.signed")}
            </Badge>
          ) : null}
        </span>
      ),
    },
    { key: "kind", label: t("repositories.settings.kind"), text: (item) => item.type ?? "—" },
    {
      key: "digest",
      label: t("repositories.settings.digest"),
      text: (item) => `${shortSha(item.digest?.replace(/^sha256:/, ""))} · ${bytesText(item.size, language)}`,
      cell: (item) => (
        <span className="font-mono">
          {shortSha(item.digest?.replace(/^sha256:/, ""))} · {bytesText(item.size, language)}
        </span>
      ),
    },
    {
      key: "published",
      label: t("repositories.settings.published"),
      kind: "date",
      text: (item) => item.createdAt ?? "",
      cell: (item) => when(item.createdAt, language),
    },
  ];
  const none = (text: string) => <p className="mt-2 text-caption text-fg-muted">{text}</p>;
  return (
    <div className="space-y-4">
      <p className="text-body text-fg-muted">{t("repositories.settings.intro")}</p>
      <Card as="section" aria-labelledby="settings-protection">
        <CardHeader as="h2" titleId="settings-protection" title={t("repositories.settings.protection")} />
        {data.protection.length === 0 ? (
          none(t("repositories.settings.noProtection"))
        ) : (
          <div className="mt-3">
            <AdminGrid<Rule>
              query={LOADED}
              caption={t("repositories.settings.protection")}
              rows={data.protection}
              columns={ruleColumns}
              rowKey={(rule) => rule.pattern ?? ""}
              storageKey={`${project}:${repository.name}:protection`}
              stack
              empty={<EmptyState bare title={t("repositories.settings.noProtection")} description={t("repositories.settings.noProtectionHint")} />}
            />
          </div>
        )}
      </Card>
      <Card as="section" aria-labelledby="settings-webhooks">
        <CardHeader as="h2" titleId="settings-webhooks" title={t("repositories.settings.webhooks")} description={t("repositories.settings.webhooksHint")} />
        {data.webhooks.length === 0 ? (
          none(t("repositories.settings.noWebhooks"))
        ) : (
          <ul className="mt-2 space-y-1.5">
            {data.webhooks.map((hook, index) => (
              <li key={index} className="flex flex-wrap items-center gap-2">
                <Badge tone={hook.active ? "success" : "neutral"}>
                  {hook.active ? t("repositories.settings.active") : t("repositories.settings.inactive")}
                </Badge>
                <span className="min-w-0 break-all font-mono text-caption">{hook.address}</span>
              </li>
            ))}
          </ul>
        )}
      </Card>
      <Card as="section" aria-labelledby="settings-collaborators">
        <CardHeader as="h2" titleId="settings-collaborators" title={t("repositories.settings.collaborators")} />
        {data.collaborators.length === 0 ? (
          none(t("repositories.settings.noCollaborators"))
        ) : (
          <ul className="mt-2 space-y-1.5">
            {data.collaborators.map((who) => (
              <li key={who.group} className="flex flex-wrap items-center gap-2 text-body">
                <span className="font-mono">{who.group}</span>
                {who.access ? <Badge>{who.access}</Badge> : null}
              </li>
            ))}
          </ul>
        )}
      </Card>
      <Card as="section" aria-labelledby="settings-packages">
        <CardHeader as="h2" titleId="settings-packages" title={t("repositories.settings.packages")} />
        {data.packages.length === 0 ? (
          none(t("repositories.settings.noPackages"))
        ) : (
          <div className="mt-3">
            <AdminGrid<Package>
              query={LOADED}
              caption={t("repositories.settings.packages")}
              rows={data.packages}
              columns={packageColumns}
              rowKey={(item) => `${item.name}-${item.version}`}
              storageKey={`${project}:${repository.name}:packages`}
              stack
              empty={<EmptyState bare title={t("repositories.settings.noPackages")} description={t("repositories.settings.noPackagesHint")} />}
            />
          </div>
        )}
      </Card>
    </div>
  );
}
