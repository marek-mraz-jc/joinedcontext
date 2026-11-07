import type { JSX, ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import {
  Alert,
  Badge,
  Card,
  EmptyState,
  PageFailed,
  PageLoading,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableRowHeaderCell,
} from "../../components/ui";
import { AccessBadge } from "../catalogue/ReadableDatasets";
import { AskAdmins } from "./AskAdmins";

type DataAccess = components["schemas"]["DataAccess"];
type AccessColumn = components["schemas"]["AccessColumn"];
type AccessCell = components["schemas"]["AccessCell"];
type AccessRow = components["schemas"]["AccessRow"];

/** The matrix, a group's column (`group`) or an Endpoint's row (`endpoint`), decided by the gateway. */
export function dataAccessQuery(narrow: { group?: string; endpoint?: string } = {}) {
  return {
    queryKey: ["organization", "data-access", narrow.group ?? null, narrow.endpoint ?? null] as const,
    queryFn: async () =>
      shaped(await unwrap(await api.GET("/api/v1/organization/data-access", { params: { query: narrow } })), "rows"),
    retry: false,
  };
}

/** What one person can read, by name only (ADR-N-067 §2.5). */
export function personReadableQuery(id: string) {
  return {
    queryKey: ["organization", "people", id, "readable"] as const,
    queryFn: async () =>
      shaped(await unwrap(await api.GET("/api/v1/organization/people/{id}/readable", { params: { path: { id } } })), "datasets"),
    retry: false,
  };
}

/** An answer whose list is missing fails as a query does, so a panel never breaks its page. */
function shaped<T extends object>(answer: T, list: string): T {
  if (!Array.isArray((answer as Record<string, unknown>)[list])) {
    throw new Error(`the server's answer holds no ${list}`);
  }
  return answer;
}

/** A column's heading: everyone in the organization, or the group. */
export function columnLabel(column: AccessColumn, t: (key: string, options?: Record<string, string>) => string): string {
  return column.kind === "organization"
    ? t("dataAccess.everyone")
    : t("dataAccess.group", { group: column.group ?? "" });
}

/**
 * A panel's loading, refusal and failure states; `children` once the answer is in. A 403 reads as
 * whose page it is, never as a failure: only org-admin and people-admin read it.
 */
function Answer({
  query,
  loading,
  children,
}: {
  query: { isPending: boolean; isError: boolean; error: unknown; refetch: () => unknown };
  loading: string;
  children: () => ReactNode;
}): JSX.Element {
  const { t } = useTranslation();
  if (query.isPending) {
    // The gateway decides each cell, which can take seconds: the wait says so in words (T-6423).
    return (
      <div className="flex flex-col gap-2">
        <p aria-hidden="true" className="text-caption text-fg-muted">
          {loading}
        </p>
        <PageLoading label={loading} lines={1} />
      </div>
    );
  }
  if (query.isError) {
    if (query.error instanceof ApiError && query.error.status === 403) {
      return <AskAdmins message={t("dataAccess.adminOnly")} />;
    }
    return <PageFailed error={query.error} onRetry={() => void query.refetch()} />;
  }
  return <>{children()}</>;
}

function Unanswered({ count }: { count: number }): JSX.Element | null {
  const { t } = useTranslation();
  return count > 0 ? <Alert tone="warning">{t("dataAccess.unanswered", { count })}</Alert> : null;
}

/** The rules behind one granted cell, each a link to propose a change of its Policy. */
function Rules({ project, cell }: { project: string; cell: AccessCell }): JSX.Element {
  const { t } = useTranslation();
  return (
    <ul className="flex flex-col gap-1">
      {cell.rules.map((rule) => (
        <li key={`${rule.policy}#${rule.rule}`} className="text-caption">
          <Link
            to="/projects/$project/$plural/$name/edit"
            params={{ project, plural: "policies", name: rule.policy }}
            className="focus-ring rounded-sm font-medium text-primary-soft-fg underline hover:no-underline [overflow-wrap:anywhere]"
            aria-label={t("dataAccess.proposeRule", { policy: rule.policy, rule: String(rule.rule) })}
          >
            {rule.rule === 0 ? rule.policy : t("dataAccess.rule", { policy: rule.policy, rule: String(rule.rule) })}
          </Link>
          <span className="mt-0.5 block text-fg-muted [overflow-wrap:anywhere]">{rule.types.join(", ")}</span>
        </li>
      ))}
    </ul>
  );
}

function Cell({ project, cell }: { project: string; cell: AccessCell }): JSX.Element {
  const { t } = useTranslation();
  if (cell.unanswered) return <span className="text-caption text-warning">{t("dataAccess.cellUnanswered")}</span>;
  if (cell.granted && cell.throughOrganization) {
    return (
      <Badge tone="neutral" className="w-fit">
        {t("dataAccess.throughEveryone")}
      </Badge>
    );
  }
  if (!cell.granted) {
    return (
      <span className="text-fg-subtle">
        <span aria-hidden="true">—</span>
        <span className="sr-only">{t("dataAccess.notGranted")}</span>
      </span>
    );
  }
  return (
    <div className="flex flex-col gap-1">
      <Badge tone="success" className="w-fit">
        {t("dataAccess.reads")}
      </Badge>
      <Rules project={project} cell={cell} />
    </div>
  );
}

function EndpointName({ row }: { row: AccessRow }): JSX.Element {
  return (
    <span className="flex min-w-0 flex-col">
      <Link
        to="/projects/$project/$plural/$name"
        params={{ project: row.project, plural: "endpoints", name: row.name }}
        className="focus-ring rounded-sm font-medium text-fg hover:underline [overflow-wrap:anywhere]"
      >
        {row.title ?? row.name}
      </Link>
      <span className="text-caption text-fg-subtle">{row.project}</span>
    </span>
  );
}

/**
 * Organization → Data access (ADR-N-067 §2.5, T-6412): every non-public Endpoint against
 * everyone in the organization and each group a Policy grants, each cell naming its rule with a
 * link that proposes a change of that Policy.
 */
export function DataAccessMatrix(): JSX.Element {
  const { t } = useTranslation();
  const matrix = useQuery(dataAccessQuery());
  return (
    <Card as="section" aria-labelledby="data-access-title" className="flex flex-col gap-3">
      <div>
        <h2 id="data-access-title" className="text-title font-semibold text-fg">
          {t("dataAccess.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("dataAccess.lead")}</p>
      </div>
      <Answer query={matrix} loading={t("dataAccess.loading")}>
        {() => <MatrixTable data={matrix.data as DataAccess} />}
      </Answer>
    </Card>
  );
}

function AllEndpoints(): JSX.Element {
  const { t } = useTranslation();
  return (
    <Link to="/organization/$tab" params={{ tab: "endpoints" }} className="text-body text-primary-soft-fg underline hover:no-underline">
      {t("dataAccess.empty.action")}
    </Link>
  );
}

export function MatrixTable({ data }: { data: DataAccess }): JSX.Element {
  const { t } = useTranslation();
  if (data.rows.length === 0) {
    return (
      <EmptyState
        icon="lock"
        bare
        title={t("dataAccess.empty.title")}
        description={t("dataAccess.empty.description")}
        action={<AllEndpoints />}
      />
    );
  }
  return (
    <>
      <Unanswered count={data.unanswered} />
      <Table caption={t("dataAccess.title")} stack>
        <TableHead>
          <TableHeaderCell>{t("dataAccess.endpoint")}</TableHeaderCell>
          {data.columns.map((column) => (
            <TableHeaderCell key={column.group ?? column.kind}>{columnLabel(column, t)}</TableHeaderCell>
          ))}
        </TableHead>
        <TableBody>
          {data.rows.map((row) => (
            <TableRow key={`${row.project}/${row.name}`}>
              <TableRowHeaderCell>
                <EndpointName row={row} />
              </TableRowHeaderCell>
              {row.cells.map((cell, index) => {
                const column = data.columns[index];
                return (
                  <TableCell key={column?.group ?? column?.kind ?? index} data-label={column ? columnLabel(column, t) : ""}>
                    <Cell project={row.project} cell={cell} />
                  </TableCell>
                );
              })}
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-3 gap-y-2 text-body text-fg-muted">
        <dt>
          <Badge tone="neutral" className="w-fit">
            {t("dataAccess.throughEveryone")}
          </Badge>
        </dt>
        <dd>{t("dataAccess.legend.throughEveryone")}</dd>
        <dt className="text-fg-subtle">
          <span aria-hidden="true">—</span>
          <span className="sr-only">{t("dataAccess.notGranted")}</span>
        </dt>
        <dd>{t("dataAccess.legend.notGranted")}</dd>
      </dl>
      <p className="text-caption text-fg-muted">{t("dataAccess.howToChange")}</p>
    </>
  );
}

/** A group page's "Datasets this group reads": the matrix's column of that group. */
export function GroupDatasets({ group }: { group: string }): JSX.Element {
  const { t } = useTranslation();
  const column = useQuery(dataAccessQuery({ group }));
  return (
    <Card as="section" padding="md" aria-labelledby="group-datasets-title" className="flex flex-col gap-3">
      <div>
        <h2 id="group-datasets-title" className="text-title font-semibold text-fg">
          {t("dataAccess.groupTitle")}
        </h2>
        <p className="text-body text-fg-muted">{t("dataAccess.groupLead", { group })}</p>
      </div>
      <Answer query={column} loading={t("dataAccess.loading")}>
        {() => {
          const data = column.data as DataAccess;
          // Column 0 is everyone; the group's own grants are those it holds beyond them.
          const read = data.rows.filter((row) => row.cells[1]?.granted && !row.cells[1].throughOrganization);
          const everyone = data.rows.filter((row) => row.cells[0]?.granted);
          return (
            <>
              <Unanswered count={data.unanswered} />
              {read.length === 0 ? (
                <EmptyState
                  icon="lock"
                  bare
                  title={t("dataAccess.groupEmpty.title", { group })}
                  description={t("dataAccess.groupEmpty.description")}
                  action={<AllEndpoints />}
                />
              ) : (
                <ul className="flex flex-col gap-3">
                  {read.map((row) => (
                    <li key={`${row.project}/${row.name}`} className="flex min-w-0 flex-col gap-1 border-b border-border pb-3 last:border-b-0 last:pb-0">
                      <EndpointName row={row} />
                      {row.cells[1] ? <Rules project={row.project} cell={row.cells[1]} /> : null}
                    </li>
                  ))}
                </ul>
              )}
              {everyone.length > 0 ? (
                <div className="flex flex-col gap-2 border-t border-border pt-3">
                  <p className="text-body text-fg-muted">{t("dataAccess.groupEveryone")}</p>
                  <ul className="flex flex-col gap-1">
                    {everyone.map((row) => (
                      <li key={`${row.project}/${row.name}`}>
                        <EndpointName row={row} />
                      </li>
                    ))}
                  </ul>
                </div>
              ) : null}
            </>
          );
        }}
      </Answer>
    </Card>
  );
}

/** The sentence naming a group that reads an Endpoint, the name a link to the group's page. */
function GroupReads({ group }: { group: string }): JSX.Element {
  const { t } = useTranslation();
  const [before, after] = t("dataAccess.endpointGroup", { group: "\0" }).split("\0");
  return (
    <span className="text-body text-fg">
      {before}
      <Link to="/organization/$tab/$" params={{ tab: "groups", _splat: encodeURIComponent(group) }} className="underline hover:no-underline">
        {group}
      </Link>
      {after}
    </span>
  );
}

/** An Endpoint page's "Who can read this", in plain words: its audience and each grant. */
export function EndpointReaders({ slug, audience }: { slug: string; audience?: string }): JSX.Element {
  const { t } = useTranslation();
  const row = useQuery({ ...dataAccessQuery({ endpoint: slug }), enabled: audience !== "public" && slug !== "" });
  if (audience === "public") {
    return <p className="text-body text-fg">{t("dataAccess.endpointPublic")}</p>;
  }
  if (slug === "") {
    return <p className="text-body text-fg">{t("dataAccess.endpointNobody")}</p>;
  }
  return (
    <Answer query={row} loading={t("dataAccess.loading")}>
      {() => {
        const data = row.data as DataAccess;
        const only = data.rows[0];
        if (!only) return <p className="text-body text-fg">{t("dataAccess.endpointNobody")}</p>;
        const readers = data.columns
          .map((column, index) => ({ column, cell: only.cells[index] }))
          .filter((reader) => reader.cell?.granted && !reader.cell.throughOrganization);
        return (
          <div className="flex flex-col gap-3">
            <Unanswered count={data.unanswered} />
            <p className="text-body text-fg">
              {only.audience === "organization"
                ? t("dataAccess.endpointAudience", { organization: only.organization ?? t("dataAccess.theOrganization") })
                : t("dataAccess.endpointProjects")}
            </p>
            {readers.length === 0 ? (
              <div className="flex flex-col gap-1">
                <p className="text-body text-fg">{t("dataAccess.endpointNobody")}</p>
                <Link
                  to="/projects/$project/$plural"
                  params={{ project: only.project, plural: "policies" }}
                  className="w-fit text-body text-primary-soft-fg underline hover:no-underline"
                >
                  {t("dataAccess.endpointNext")}
                </Link>
              </div>
            ) : (
              <ul className="flex flex-col gap-3">
                {readers.map(({ column, cell }) => (
                  <li key={column.group ?? column.kind} className="flex flex-col gap-2">
                    {column.kind === "organization" ? (
                      <span className="text-body text-fg">{t("dataAccess.endpointEveryone")}</span>
                    ) : (
                      <GroupReads group={column.group ?? ""} />
                    )}
                    {cell ? <Rules project={only.project} cell={cell} /> : null}
                  </li>
                ))}
              </ul>
            )}
          </div>
        );
      }}
    </Answer>
  );
}

/** A person page's "What this person can read": the catalogue as they would see it, names only. */
export function PersonReadable({ id, name }: { id: string; name: string }): JSX.Element {
  const { t } = useTranslation();
  const readable = useQuery(personReadableQuery(id));
  return (
    <Card as="section" padding="md" aria-labelledby="person-readable-title" className="flex flex-col gap-3">
      <div>
        <h2 id="person-readable-title" className="text-title font-semibold text-fg">
          {t("dataAccess.personTitle")}
        </h2>
        <p className="text-body text-fg-muted">{t("dataAccess.personLead", { name })}</p>
      </div>
      <Answer query={readable} loading={t("dataAccess.loading")}>
        {() => {
          const data = readable.data as components["schemas"]["ReadableCatalogue"];
          const internal = data.datasets.filter((dataset) => dataset.access !== "public");
          const open = data.datasets.length - internal.length;
          return (
            <>
              <Unanswered count={data.unanswered} />
              {internal.length === 0 ? (
                <EmptyState icon="lock" bare title={t("dataAccess.personEmpty.title", { name })} description={t("dataAccess.personEmpty.description")} />
              ) : (
                <ul className="flex flex-col gap-2">
                  {internal.map((dataset) => (
                    <li key={dataset.slug} className="flex min-w-0 flex-wrap items-center gap-2">
                      <span className="text-body font-medium text-fg [overflow-wrap:anywhere]">{dataset.title ?? dataset.name}</span>
                      <AccessBadge dataset={dataset} words="dataAccess.personAccess" />
                      <span className="text-caption text-fg-subtle">{dataset.project}</span>
                    </li>
                  ))}
                </ul>
              )}
              {open > 0 ? <p className="text-caption text-fg-muted">{t("dataAccess.personPublic", { count: open })}</p> : null}
            </>
          );
        }}
      </Answer>
    </Card>
  );
}
