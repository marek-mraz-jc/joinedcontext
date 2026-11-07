import { useId } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { api, unwrap } from "../../api/client";
import { PlanDiffViewer } from "../../components/diff/PlanDiffViewer";
import type { FieldChange } from "../../components/diff/PlanDiffViewer";
import { Alert, Badge, buttonClass, Card, EmptyState, PageFailed, PageHeader, PageLoading } from "../../components/ui";
import type { components } from "../../api/schema";

type Comparison = components["schemas"]["Comparison"];
type ChangeFile = components["schemas"]["ChangeFile"];

const OPERATION_ORDER = ["Create", "Update", "Delete"] as const;

function operationOrder(op: string): number {
  return OPERATION_ORDER.indexOf(op as (typeof OPERATION_ORDER)[number]);
}

/**
 * The resource a file holds, by its name: `projects/{project}/{plural}/{name}.yaml`, or
 * `projects/{project}/{plural}/{name}/pipeline.yaml` for a kind kept in a folder of its own, which
 * read as "pipeline" for every pipeline (T-4868).
 */
function fileName(path: string): string {
  const segments = path.split("/");
  const name = segments[0] === "projects" && segments.length > 4 ? segments[3] : segments[segments.length - 1];
  return (name ?? path).replace(/\.ya?ml$/, "");
}

/** The list of files from a comparison, grouped by kind and ordered by operation. */
export function FileList({ files }: { files: ChangeFile[] }): React.JSX.Element {
  const { t } = useTranslation();
  const headingIds = useId();
  const grouped: Record<string, ChangeFile[]> = {};
  for (const file of files) {
    const kind = file.kind ?? "unknown";
    if (!grouped[kind]) grouped[kind] = [];
    grouped[kind].push(file);
  }
  const sortedKinds = Object.keys(grouped).sort();
  return (
    <div className="space-y-6">
      {sortedKinds.map((kind) => {
        const kindFiles = grouped[kind].sort(
          (a, b) => operationOrder(a.operation) - operationOrder(b.operation),
        );
        return (
          // The page's own H1 is the heading above this list, so each kind is an H2: an H3 here
          // skipped a level and a screen reader's heading list read the groups as belonging to
          // a section that was never there (UI-16).
          <section key={kind} aria-labelledby={`${headingIds}-${kind}`}>
            <h2 id={`${headingIds}-${kind}`} className="text-body font-semibold text-fg mb-2">
              {kind}
            </h2>
            <ul className="space-y-3">
              {kindFiles.map((file) => (
                <Card as="li" padding="sm" key={file.path}>
                  <div className="flex flex-wrap items-center gap-2 mb-2">
                    <span className="font-mono text-body font-medium text-fg">
                      {fileName(file.path)}
                    </span>
                    <Badge tone="neutral" mono>
                      {file.kind}
                    </Badge>
                    <Badge
                      tone={
                        file.lane === "red"
                          ? "danger"
                          : file.lane === "yellow"
                            ? "warning"
                            : "success"
                      }
                    >
                      {t(`lane.${file.lane}`)}
                    </Badge>
                    <Badge
                      tone={
                        file.operation === "Create"
                          ? "success"
                          : file.operation === "Delete"
                            ? "danger"
                            : "info"
                      }
                    >
                      {file.operation === "Create"
                        ? t("approvals.diffAdded")
                        : file.operation === "Delete"
                          ? t("approvals.diffRemoved")
                          : t("approvals.diffChanged")}
                    </Badge>
                  </div>
                  {file.fields && file.fields.length > 0 ? (
                    <details className="text-body">
                      <summary className="cursor-pointer text-fg-muted hover:text-fg">
                        {t("workspaces.compare.showDiff")}
                      </summary>
                      <div className="mt-2">
                        <PlanDiffViewer fields={file.fields as FieldChange[]} />
                      </div>
                    </details>
                  ) : null}
                </Card>
              ))}
            </ul>
          </section>
        );
      })}
    </div>
  );
}

export function ComparePage({
  project,
  name,
}: {
  project: string;
  name: string;
}): React.JSX.Element {
  const { t } = useTranslation();
  const compareQuery = useQuery({
    queryKey: ["workspaces", project, name, "compare"],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/workspaces/{name}/compare", {
          params: { path: { project, name } },
        }),
      ),
  });

  const comparison = compareQuery.data as Comparison | undefined;
  const files = comparison?.files ?? [];
  const conflicts = comparison?.conflicts ?? [];

  const added = files.filter((f) => f.operation === "Create").length;
  const changed = files.filter((f) => f.operation === "Update").length;
  const removed = files.filter((f) => f.operation === "Delete").length;

  // The heading stands in all three states, so the page does not appear out of nothing when the
  // comparison arrives and a failure is still a page one can act on (UI-15, UI-16).
  // The summary counts nothing until there is something to count: "0 added, 0 changed,
  // 0 removed" under a heading is an answer, and while the comparison is on its way it is the
  // wrong one.
  // Which copy it compares (T-2760): the page said "What the copy changes" of no copy in
  // particular. The Shell's trail above it reads project › Copies › the copy, the way back.
  const header = (
    <PageHeader
      title={t("workspaces.compare.titleOf", { name })}
      description={
        comparison ? t("workspaces.compare.summary", { added, changed, removed }) : undefined
      }
      actions={
        // The next step after reading what the copy changes (T-4868): it was only in the copy's
        // bar, which a person reading this page had left behind.
        files.length > 0 ? (
          <Link
            to="/projects/$project/workspaces/$name/bring-back"
            params={{ project, name }}
            className={buttonClass("primary", "md")}
          >
            {t("workspaces.bar.bringBack")}
          </Link>
        ) : undefined
      }
    />
  );

  if (compareQuery.isPending) {
    return (
      <section aria-label={t("workspaces.compare.title")} className="space-y-6">
        {header}
        <PageLoading label={t("app.loading")} />
      </section>
    );
  }
  if (compareQuery.isError) {
    return (
      <section aria-label={t("workspaces.compare.title")} className="space-y-6">
        {header}
        <PageFailed
          error={compareQuery.error}
          onRetry={() => {
            void compareQuery.refetch();
          }}
        />
      </section>
    );
  }

  return (
    <section aria-label={t("workspaces.compare.title")} className="space-y-6">
      {header}
      {conflicts.length > 0 ? (
        <Alert tone="warning" role="status">
          {t("workspaces.compare.conflicts", { count: conflicts.length })}
          <Link
            to="/projects/$project/workspaces/$name/bring-back"
            params={{ project, name }}
            className="ml-2 underline"
          >
            {t("workspaces.compare.resolveConflicts")}
          </Link>
        </Alert>
      ) : null}
      {files.length === 0 ? (
        <EmptyState
          title={t("workspaces.compare.empty")}
          description={t("workspaces.compare.emptyHint")}
          action={
            <Link
              to="/projects/$project/workspaces"
              params={{ project }}
              className={buttonClass("secondary", "md")}
            >
              {t("workspaces.compare.back")}
            </Link>
          }
        />
      ) : (
        <FileList files={files} />
      )}
    </section>
  );
}
