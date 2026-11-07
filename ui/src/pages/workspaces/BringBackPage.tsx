import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { FileList } from "./ComparePage";
import { Alert, Button, EmptyState, PageHeader, PageLoading, RadioGroup } from "../../components/ui";
import { ResourcePageFailed } from "../../components/ui/PageState";
import { useAuth } from "../../auth/AuthProvider";


function valueText(value: unknown): string {
  if (typeof value === "string") return value;
  return JSON.stringify(value);
}

export function BringBackPage({
  project,
  name,
}: {
  project: string;
  name: string;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "sk";
  const { identity } = useAuth();
  const queryClient = useQueryClient();
  const [resolutions, setResolutions] = useState<
    Record<string, "ours" | "theirs">
  >({});
  const [updateError, setUpdateError] = useState<string | null>(null);
  /** How many files the last update took in from the project; `null` before the first. */
  const [updatedFiles, setUpdatedFiles] = useState<number | null>(null);
  const [proposeError, setProposeError] = useState<string | null>(null);
  const [proposedChange, setProposedChange] = useState<string | null>(null);

  const workspace = useQuery({
    queryKey: [...queryKeys.list(project, "workspaces"), name],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/workspaces/{name}", {
          params: { path: { project, name } },
        }),
      ),
    retry: false,
  });

  const comparison = useQuery({
    queryKey: [...queryKeys.list(project, "workspaces"), name, "compare"],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/workspaces/{name}/compare", {
          params: { path: { project, name } },
        }),
      ),
  });

  const isOwner =
    identity &&
    (identity.email === workspace.data?.owner ||
      identity.username === workspace.data?.owner);

  const update = useMutation({
    mutationFn: async () => {
      const body: { resolutions: { path: string; field: string; keep: "ours" | "theirs" }[] } = {
        resolutions: Object.entries(resolutions).map(([key, keep]) => {
          const [path, field] = key.split("::");
          return { path, field, keep };
        }),
      };
      return unwrap(
        await api.POST(
          "/api/v1/projects/{project}/workspaces/{name}/update",
          {
            params: { path: { project, name } },
            body,
          },
        ),
      );
    },
    onSuccess: (report) => {
      setUpdateError(null);
      // What the update did, in words: a click that changes nothing on screen reads as broken.
      setUpdatedFiles(report.taken.length + report.merged.length);
      // The merge moved the base: what was chosen answered the conflicts that are gone now.
      setResolutions({});
      void queryClient.invalidateQueries({
        queryKey: [...queryKeys.list(project, "workspaces"), name, "compare"],
      });
    },
    onError: (err: Error) => setUpdateError(err.message),
  });

  const propose = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST(
          "/api/v1/projects/{project}/workspaces/{name}/propose",
          {
            params: { path: { project, name } },
          },
        ),
      ),
    onSuccess: (data) => {
      const changeId = (data as { metadata?: { name?: string } })?.metadata?.name;
      if (changeId) setProposedChange(changeId);
    },
    onError: (err: Error) => setProposeError(err.message),
  });

  const files = comparison.data?.files ?? [];
  const conflicts = comparison.data?.conflicts ?? [];

  // Determine riskiest lane
  const laneOrder = ["red", "yellow", "green"];
  let riskiestLane: string | undefined;
  for (const file of files) {
    if (file.lane && (!riskiestLane || laneOrder.indexOf(file.lane) < laneOrder.indexOf(riskiestLane))) {
      riskiestLane = file.lane;
    }
  }

  // Count total fields that need resolution
  const totalConflictFields = conflicts.reduce(
    (sum, c) => sum + c.fields.length,
    0,
  );
  const resolvedCount = Object.keys(resolutions).length;
  const allResolved = totalConflictFields === 0 || resolvedCount >= totalConflictFields;

  // The heading stands while the copy and its comparison are read and when either could not be:
  // a refusal, an outage and a copy that is gone used to be one red line of the same words with
  // nothing to press (UI-15, UI-16, UI-44).
  const header = <PageHeader title={t("workspaces.bringBack.title")} description={t("workspaces.lead")} />;

  if (workspace.isPending || comparison.isPending) {
    return (
      <section aria-label={t("workspaces.bringBack.title")} className="space-y-6">
        {header}
        <PageLoading label={t("app.loading")} />
      </section>
    );
  }

  if (workspace.isError || comparison.isError) {
    const failed = workspace.isError ? workspace : comparison;
    return (
      <section aria-label={t("workspaces.bringBack.title")}>
        <ResourcePageFailed
          title={t("workspaces.bringBack.title")}
          description={t("workspaces.lead")}
          error={failed.error}
          onRetry={() => {
            void failed.refetch();
          }}
          back={
            <Link to="/projects/$project/workspaces" params={{ project }} className="focus-ring text-body text-primary-soft-fg underline hover:no-underline">
              {t("workspaces.compare.back")}
            </Link>
          }
        />
      </section>
    );
  }

  const { title, owner, createdAt } = workspace.data;
  const display = title ?? name;
  const date = new Date(createdAt).toLocaleDateString(locale);

  return (
    <section aria-label={t("workspaces.bringBack.title")} className="space-y-6">
      <PageHeader
        title={t("workspaces.bringBack.title")}
        description={t("workspaces.bringBack.from", { name: display, owner, date })}
      />

      {riskiestLane ? (
        <p className="text-body">
          {t(`workspaces.bringBack.lane.${riskiestLane}`)}
        </p>
      ) : null}

      {proposedChange ? (
        <Alert tone="success" role="status">
          {t("workspaces.bringBack.proposed")}{" "}
          <Link
            to="/projects/$project/approvals/$id"
            params={{ project, id: proposedChange }}
            className="underline"
          >
            {proposedChange}
          </Link>
        </Alert>
      ) : null}

      {proposeError ? (
        <Alert tone="danger" role="alert">
          {proposeError}
        </Alert>
      ) : null}

      {updatedFiles !== null ? (
        <Alert tone="success" role="status">
          {t("workspaces.bringBack.updated", { count: updatedFiles })}
        </Alert>
      ) : null}

      {updateError ? (
        <Alert tone="danger" role="alert">
          {updateError}
        </Alert>
      ) : null}

      {!isOwner ? (
        <p className="text-body text-fg-muted">
          {t("workspaces.bringBack.notOwner")}
        </p>
      ) : null}

      {files.length === 0 ? (
        <EmptyState title={t("workspaces.compare.empty")} />
      ) : (
        <FileList files={files} />
      )}

      {conflicts.length > 0 ? (
        <div className="space-y-6">
          <h2 className="text-title font-semibold">
            {t("workspaces.bringBack.conflicts")}
          </h2>
          {conflicts.map((conflict, idx) => (
            <div
              key={`${conflict.path}-${idx}`}
              className="rounded-lg border border-warning/30 bg-warning-soft p-4"
            >
              <p className="font-mono text-body font-medium mb-2">
                {conflict.path}
              </p>
              {conflict.fields.length === 0 ? (
                <p className="text-body">{t("workspaces.bringBack.mergeable")}</p>
              ) : (
                <div className="space-y-3">
                  {conflict.fields.map((field) => {
                    const key = `${conflict.path}::${field.path}`;
                    return (
                      <RadioGroup
                        key={key}
                        name={key}
                        layout="row"
                        legend={
                          field.path === ""
                            ? t("workspaces.bringBack.wholeFile")
                            : field.path
                        }
                        value={resolutions[key]}
                        options={[
                          {
                            value: "ours",
                            label: t("workspaces.bringBack.keepOurs"),
                            description: valueText(field.ours),
                          },
                          {
                            value: "theirs",
                            label: t("workspaces.bringBack.takeTheirs"),
                            description: valueText(field.theirs),
                          },
                        ]}
                        onChange={(choice) =>
                          setResolutions((prev) => ({ ...prev, [key]: choice }))
                        }
                        className="border-t border-border pt-2"
                      />
                    );
                  })}
                </div>
              )}
            </div>
          ))}
        </div>
      ) : null}

      {isOwner ? (
        <div className="flex flex-wrap gap-3">
          {/*
            Refused with a reason rather than hard-disabled: a `disabled` button leaves the tab
            order, so the sentence saying what is still missing could never be read by the person
            most likely to need it (T-1743, UI-44). The click is refused either way.
          */}
          <Button
            variant="secondary"
            disabled={!allResolved || update.isPending}
            disabledReason={allResolved ? undefined : t("workspaces.bringBack.updateBlocked")}
            loading={update.isPending}
            onClick={() => update.mutate()}
          >
            {t("workspaces.bringBack.update")}
          </Button>
          <Button
            variant="primary"
            disabled={conflicts.length > 0 || propose.isPending}
            disabledReason={
              conflicts.length > 0 ? t("workspaces.bringBack.proposeBlocked") : undefined
            }
            loading={propose.isPending}
            onClick={() => propose.mutate()}
          >
            {t("workspaces.bringBack.propose")}
          </Button>
        </div>
      ) : null}
    </section>
  );
}
