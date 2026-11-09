import { useState } from "react";
import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { Badge, Button, Card, CardHeader, EmptyState, Field, PageFailed, PageLoading, Select } from "../../components/ui";
import { Prose } from "../apps/Prose";
import { shortSha, useRepositoryRead } from "./api";
import type { Page, Pull, PullRow, Repository } from "./api";
import { StatusBadge, when } from "./CommitsView";
import { RepoLink } from "./links";
import type { RepositorySearch } from "./links";
import { Pager } from "./RepositoriesPage";

const PAGE = 30;
const STATES = ["open", "closed", "all"] as const;
type PullState = (typeof STATES)[number];

const STATE_TONE = { open: "success", closed: "neutral", merged: "accent" } as const;

export function PullStateBadge({ state }: { state: PullRow["state"] }): JSX.Element {
  const { t } = useTranslation();
  return <Badge tone={STATE_TONE[state] ?? "neutral"}>{t(`repositories.pulls.state.${state}`)}</Badge>;
}

/** The pull requests tab: the list by state, or one pull request when the address names it. */
export function PullsView({
  project,
  repository,
  search,
}: {
  project: string;
  repository: Repository;
  search: RepositorySearch;
}): JSX.Element {
  if (search.pull !== undefined) {
    return <PullDetail project={project} repository={repository.name} number={search.pull} />;
  }
  return <PullList project={project} repository={repository.name} />;
}

export function PullList({ project, repository }: { project: string; repository: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const [state, setState] = useState<PullState>("open");
  const [cursor, setCursor] = useState(0);
  const pulls = useRepositoryRead<Page<PullRow>>(project, "jc_repository_pulls", { repository, state, cursor, limit: PAGE });
  const language = i18n.resolvedLanguage ?? "en";
  return (
    <div className="space-y-4">
      <Field id="repository-pull-state" label={t("repositories.pulls.filter.label")} help={t("repositories.pulls.filter.help")} className="max-w-xs">
        <Select
          id="repository-pull-state"
          value={state}
          onChange={(event) => {
            setState(event.target.value as PullState);
            setCursor(0);
          }}
        >
          {STATES.map((value) => (
            <option key={value} value={value}>
              {t(`repositories.pulls.filter.${value}`)}
            </option>
          ))}
        </Select>
      </Field>
      {pulls.isError ? (
        <PageFailed error={pulls.error} onRetry={() => void pulls.refetch()} />
      ) : pulls.isPending ? (
        <PageLoading label={t("repositories.loading")} />
      ) : pulls.data.items.length === 0 ? (
        <EmptyState
          icon="git"
          title={t(`repositories.pulls.emptyState.${state}`)}
          description={t("repositories.pulls.emptyHint")}
          action={
            state === "all" ? undefined : (
              <Button size="sm" variant="secondary" onClick={() => {
                setState("all");
                setCursor(0);
              }}>
                {t("repositories.pulls.showAll")}
              </Button>
            )
          }
        />
      ) : (
        <>
          <Card as="ul" flush className="divide-y divide-border overflow-hidden">
            {pulls.data.items.map((pull) => (
              <li key={pull.number} className="flex flex-wrap items-baseline gap-x-3 gap-y-1 px-4 py-3">
                <span className="w-20 shrink-0">
                  <PullStateBadge state={pull.state} />
                </span>
                <div className="min-w-0 flex-1">
                  <RepoLink
                    project={project}
                    name={repository}
                    search={{ tab: "pulls", pull: pull.number }}
                    className="focus-ring break-words rounded-sm font-semibold text-primary-soft-fg underline-offset-2 hover:underline"
                  >
                    {pull.title}
                  </RepoLink>
                  <p className="text-caption text-fg-muted">
                    {t("repositories.pulls.line", {
                      number: pull.number,
                      author: pull.author ?? "—",
                      date: when(pull.createdAt, language),
                      head: pull.head ?? "—",
                      base: pull.base ?? "—",
                    })}
                  </p>
                </div>
              </li>
            ))}
          </Card>
          <Pager total={pulls.data.total} cursor={cursor} size={PAGE} next={pulls.data.nextCursor} onCursor={setCursor} />
        </>
      )}
    </div>
  );
}

export function PullDetail({ project, repository, number }: { project: string; repository: string; number: number }): JSX.Element {
  const { t, i18n } = useTranslation();
  const pull = useRepositoryRead<Pull>(project, "jc_repository_pull", { repository, number });
  if (pull.isError) return <PageFailed error={pull.error} onRetry={() => void pull.refetch()} />;
  if (pull.isPending) return <PageLoading label={t("repositories.loading")} />;
  const data = pull.data;
  return (
    <div className="space-y-4">
      <RepoLink project={project} name={repository} search={{ tab: "pulls" }}>
        {t("repositories.pulls.all")}
      </RepoLink>
      <Card as="section" aria-labelledby="pull-title" className="space-y-3">
        <div className="flex flex-wrap items-center gap-2">
          <PullStateBadge state={data.state} />
          <h2 id="pull-title" className="min-w-0 break-words text-title font-semibold">
            {data.title}
          </h2>
        </div>
        <p className="text-caption text-fg-muted">
          {t("repositories.pulls.line", {
            number: data.number,
            author: data.author ?? "—",
            date: when(data.createdAt, i18n.resolvedLanguage ?? "en"),
            head: data.head ?? "—",
            base: data.base ?? "—",
          })}
        </p>
        {data.change ? (
          <p className="text-body">
            {t("repositories.pulls.change")}{" "}
            <Link
              to="/projects/$project/approvals/$id"
              params={{ project, id: data.change }}
              className="focus-ring rounded-sm font-semibold text-primary-soft-fg underline"
            >
              {t("repositories.pulls.openChange")}
            </Link>
          </p>
        ) : null}
        {data.body?.trim() ? (
          <div className="text-body">
            <Prose text={data.body} />
          </div>
        ) : (
          <p className="text-caption text-fg-muted">{t("repositories.pulls.noBody")}</p>
        )}
      </Card>
      <Card as="section" aria-labelledby="pull-files">
        <CardHeader as="h2" titleId="pull-files" title={t("repositories.pulls.files", { count: data.files.length })} />
        {data.files.length === 0 ? (
          <p className="mt-2 text-caption text-fg-muted">{t("repositories.pulls.noFiles")}</p>
        ) : (
          <ul className="mt-2 space-y-1.5">
            {data.files.map((file) => (
              <li key={file.path} className="flex flex-wrap items-center gap-2">
                <StatusBadge status={file.status} />
                <span className="min-w-0 break-all font-mono text-caption">{file.path}</span>
              </li>
            ))}
          </ul>
        )}
      </Card>
      <Card as="section" aria-labelledby="pull-reviews">
        <CardHeader as="h2" titleId="pull-reviews" title={t("repositories.pulls.reviews")} />
        {data.reviews.length === 0 ? (
          <p className="mt-2 text-caption text-fg-muted">{t("repositories.pulls.noReviews")}</p>
        ) : (
          <ul className="mt-2 divide-y divide-border">
            {data.reviews.map((review, index) => (
              <li key={index} className="space-y-1 py-2">
                <p className="flex flex-wrap items-center gap-2 text-body">
                  <span className="font-semibold">{review.author ?? "—"}</span>
                  <Badge tone={review.verdict === "APPROVED" ? "success" : review.verdict === "REQUEST_CHANGES" ? "danger" : "neutral"}>
                    {t(`repositories.pulls.verdict.${review.verdict === "APPROVED" || review.verdict === "REQUEST_CHANGES" ? review.verdict : "COMMENT"}`)}
                  </Badge>
                  {review.commit ? <code className="font-mono text-caption text-fg-muted">{shortSha(review.commit)}</code> : null}
                </p>
                {review.body ? <Prose text={review.body} /> : null}
              </li>
            ))}
          </ul>
        )}
      </Card>
    </div>
  );
}
