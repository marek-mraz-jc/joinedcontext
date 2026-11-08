import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import {
  Badge,
  Card,
  CardHeader,
  PageFailed,
  PageLoading,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
} from "../../components/ui";
import { bytesText } from "../knowledge/knowledge";
import { shortSha, useRepositoryRead } from "./api";
import type { Repository, RepositorySettings } from "./api";
import { when } from "./CommitsView";

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
            <Table caption={t("repositories.settings.protection")} stack>
              <TableHead>
                <TableHeaderCell>{t("repositories.settings.pattern")}</TableHeaderCell>
                <TableHeaderCell>{t("repositories.settings.push")}</TableHeaderCell>
                <TableHeaderCell>{t("repositories.settings.forcePush")}</TableHeaderCell>
                <TableHeaderCell>{t("repositories.settings.approvals")}</TableHeaderCell>
              </TableHead>
              <TableBody>
                {data.protection.map((rule) => (
                  <TableRow key={rule.pattern}>
                    <TableCell className="font-mono">{rule.pattern}</TableCell>
                    <TableCell data-label={t("repositories.settings.push")}>{yes(rule.pushAllowed)}</TableCell>
                    <TableCell data-label={t("repositories.settings.forcePush")}>{yes(rule.forcePushAllowed)}</TableCell>
                    <TableCell data-label={t("repositories.settings.approvals")}>{rule.requiredApprovals ?? 0}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
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
            <Table caption={t("repositories.settings.packages")} stack>
              <TableHead>
                <TableHeaderCell>{t("repositories.settings.version")}</TableHeaderCell>
                <TableHeaderCell>{t("repositories.settings.kind")}</TableHeaderCell>
                <TableHeaderCell>{t("repositories.settings.digest")}</TableHeaderCell>
                <TableHeaderCell>{t("repositories.settings.published")}</TableHeaderCell>
              </TableHead>
              <TableBody>
                {data.packages.map((item) => (
                  <TableRow key={`${item.name}-${item.version}`}>
                    <TableCell className="font-mono">
                      {item.name} {item.version}
                      {item.signed ? (
                        <Badge tone="success" className="ml-1">
                          {t("repositories.settings.signed")}
                        </Badge>
                      ) : null}
                    </TableCell>
                    <TableCell data-label={t("repositories.settings.kind")}>{item.type ?? "—"}</TableCell>
                    <TableCell data-label={t("repositories.settings.digest")} className="font-mono">
                      {shortSha(item.digest?.replace(/^sha256:/, ""))} · {bytesText(item.size, language)}
                    </TableCell>
                    <TableCell data-label={t("repositories.settings.published")}>{when(item.createdAt, language)}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
      </Card>
    </div>
  );
}
