import { useState } from "react";
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import {
  Badge,
  Button,
  EmptyState,
  Field,
  Input,
  PageFailed,
  PageHeader,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  PageLoading,
} from "../../components/ui";
import { bytesText } from "../knowledge/knowledge";
import { isNotFound, useRepositoryRead } from "./api";
import type { Page, RepositoryRole, RepositorySummary } from "./api";

const PAGE = 30;

export function RoleBadge({ role }: { role: RepositoryRole }): JSX.Element {
  const { t } = useTranslation();
  const tone = role === "configuration" ? "primary" : role === "application" ? "accent" : "neutral";
  return <Badge tone={tone}>{t(`repositories.role.${role}`)}</Badge>;
}

/**
 * The repositories a project owns on the forge (shako): its configuration, its own and its
 * applications', with a filter and pages of thirty. Each opens its own page.
 */
export function RepositoriesPage({ project }: { project: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const [filter, setFilter] = useState("");
  const [cursor, setCursor] = useState(0);
  const input = { ...(filter.trim() ? { filter: filter.trim() } : {}), cursor, limit: PAGE };
  const list = useRepositoryRead<Page<RepositorySummary>>(project, "jc_repository_list", input);
  const items = list.data?.items ?? [];
  const language = i18n.resolvedLanguage ?? "en";

  return (
    <section aria-labelledby="repositories-title" className="space-y-6">
      <PageHeader title={<span id="repositories-title">{t("repositories.title")}</span>} description={t("repositories.intro")} />
      {list.isError || (items.length === 0 && !filter.trim() && !list.isPending) ? null : (
      <Field id="repositories-filter" label={t("repositories.filter.label")} help={t("repositories.filter.help")} className="max-w-md">
        <Input
          id="repositories-filter"
          type="search"
          value={filter}
          maxLength={100}
          onChange={(event) => {
            setFilter(event.target.value);
            setCursor(0);
          }}
        />
      </Field>
      )}
      {list.isError ? (
        <PageFailed error={list.error} onRetry={() => void list.refetch()}>
          {isNotFound(list.error) ? t("repositories.refused.list", { project }) : undefined}
        </PageFailed>
      ) : list.isPending ? (
        <PageLoading label={t("repositories.loading")} />
      ) : items.length === 0 ? (
        <EmptyState
          icon="git"
          title={filter.trim() ? t("repositories.empty.filtered") : t("repositories.empty.title")}
          description={filter.trim() ? t("repositories.empty.filteredHint") : t("repositories.empty.description")}
        />
      ) : (
        <>
          <Table caption={t("repositories.title")} stack>
            <TableHead>
              <TableHeaderCell>{t("repositories.column.name")}</TableHeaderCell>
              <TableHeaderCell>{t("repositories.column.role")}</TableHeaderCell>
              <TableHeaderCell>{t("repositories.column.branch")}</TableHeaderCell>
              <TableHeaderCell>{t("repositories.column.size")}</TableHeaderCell>
            </TableHead>
            <TableBody>
              {items.map((repository) => (
                <TableRow key={repository.name}>
                  <TableCell>
                    <Link
                      to="/projects/$project/repositories/$name"
                      params={{ project, name: repository.name }}
                      className="focus-ring rounded-sm font-mono font-semibold text-primary-soft-fg underline-offset-2 hover:underline"
                    >
                      {repository.name}
                    </Link>
                    {repository.description ? <p className="mt-0.5 text-caption text-fg-muted">{repository.description}</p> : null}
                  </TableCell>
                  <TableCell data-label={t("repositories.column.role")}>
                    <span className="inline-flex flex-wrap items-center gap-1">
                      <RoleBadge role={repository.role} />
                      {repository.archived ? <Badge tone="warning">{t("repositories.archived")}</Badge> : null}
                    </span>
                  </TableCell>
                  <TableCell data-label={t("repositories.column.branch")} className="font-mono">
                    {repository.defaultBranch}
                  </TableCell>
                  <TableCell data-label={t("repositories.column.size")}>{bytesText(repository.sizeBytes, language)}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
          <Pager total={list.data?.total ?? 0} cursor={cursor} size={PAGE} onCursor={setCursor} next={list.data?.nextCursor} />
        </>
      )}
    </section>
  );
}

/** Previous and next over a paged read, with where this page stands. */
export function Pager({
  total,
  cursor,
  size,
  next,
  onCursor,
}: {
  total: number;
  cursor: number;
  size: number;
  next?: number;
  onCursor: (cursor: number) => void;
}): JSX.Element | null {
  const { t } = useTranslation();
  if (cursor === 0 && next === undefined) return null;
  return (
    <nav aria-label={t("repositories.pager.label")} className="flex flex-wrap items-center gap-2">
      <Button size="sm" variant="secondary" disabled={cursor === 0} disabledReason={t("repositories.pager.firstPage")} onClick={() => onCursor(Math.max(0, cursor - size))}>
        {t("repositories.pager.previous")}
      </Button>
      <span className="text-caption text-fg-muted">
        {t("repositories.pager.position", { from: cursor + 1, to: Math.min(cursor + size, total), total })}
      </span>
      <Button size="sm" variant="secondary" disabled={next === undefined} disabledReason={t("repositories.pager.lastPage")} onClick={() => next !== undefined && onCursor(next)}>
        {t("repositories.pager.next")}
      </Button>
    </nav>
  );
}
