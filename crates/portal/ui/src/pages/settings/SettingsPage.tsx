import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { useAdministers } from "../../api/permissions";
import { Card, Icon, PageHeader } from "../../components/ui";
import type { IconName } from "../../components/ui/icons";

interface Entry {
  key: string;
  icon: IconName;
  link: JSX.Element;
}

/**
 * Every setting a person reaches, by whose it is (owner 2026-11-01): their own (AI connectors),
 * the project's, and the platform's for a platform administrator. The account menu opens it. A
 * member with no project (ADR-N-067) gets their own settings alone.
 */
export function SettingsPage({ project }: { project?: string }): JSX.Element {
  const { t } = useTranslation();
  const { opensAdministration } = useAdministers();
  const groups: { scope: "personal" | "project" | "platform"; entries: Entry[] }[] = [
    {
      scope: "personal",
      entries: [{ key: "aiConnectors", icon: "access", link: <Link to="/ai-connectors">{t("aiConnectors.title")}</Link> }],
    },
    ...(project === undefined
      ? []
      : [
          {
            scope: "project" as const,
            entries: [
              {
                key: "project",
                icon: "spaces" as IconName,
                link: (
                  <Link to="/projects/$project/settings" params={{ project }}>
                    {t("settingsPage.projectSettings", { project })}
                  </Link>
                ),
              },
            ],
          },
        ]),
    ...(opensAdministration
      ? [
          {
            scope: "platform" as const,
            entries: [
              {
                key: "administration",
                icon: "access" as IconName,
                link: (
                  <Link to="/organization/$tab" params={{ tab: "settings" }}>
                    {t("nav.organization")}
                  </Link>
                ),
              },
            ],
          },
        ]
      : []),
  ];
  return (
    <div className="flex flex-col gap-6">
      <PageHeader title={t("settingsPage.title")} description={t(opensAdministration ? "settingsPage.description" : project === undefined ? "settingsPage.descriptionOwn" : "settingsPage.descriptionMember")} />
      {/* Side by side on a wide screen: one link per card stretched across 1440 px read as empty. */}
      <div className="grid gap-6 md:grid-cols-2 xl:grid-cols-3">
        {groups.map((group) => (
          <Card key={group.scope}>
            <h2 className="text-sm font-semibold uppercase tracking-wide text-fg-muted">{t(`settingsPage.scope.${group.scope}`)}</h2>
            <p className="mb-3 text-caption text-fg-muted">{t(`settingsPage.scopeHint.${group.scope}`)}</p>
            <ul className="flex flex-col gap-2">
              {group.entries.map((entry) => (
                <li key={entry.key} className="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-body font-medium [&_a]:text-primary-soft-fg [&_a]:underline-offset-2 hover:[&_a]:underline">
                  <Icon name={entry.icon} className="size-4 text-fg-subtle" />
                  {entry.link}
                  <span className="text-caption font-normal text-fg-muted">{t(`settingsPage.hint.${entry.key}`)}</span>
                </li>
              ))}
            </ul>
          </Card>
        ))}
      </div>
    </div>
  );
}
