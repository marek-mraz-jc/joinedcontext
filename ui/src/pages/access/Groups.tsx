import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import { proposeChecked } from "../../api/proposal";
import { asManifests, isChange, ORG_NAMESPACE, storedMetadata } from "../../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { EditResourceAction } from "../../components/EditResourceDialog";
import { ResourceFormDialog } from "../../components/ResourceFormDialog";
import { FormFrame, useCreateForm, useFormRoute } from "../../components/forms/FormRoute";
import { groupSchema } from "../../schemas/kinds";
import { personLabel, usePersonNames } from "../../api/personNames";
import {
  Badge,
  Button,
  EmptyState,
  PageFailed,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
} from "../../components/ui";
import { PermissionGuard } from "../../components/ui/PermissionGuard";

interface Member {
  user?: string;
}

/** One group of the organization, with what the last reconcile said about the realm. */
interface Row {
  name: string;
  description: string;
  members: string[];
  /** The reconciler's word: what it had to correct, or whom the realm does not know yet. */
  sync?: { reason: string; message: string; ok: boolean };
}

/** What the group form holds: the name it is filed under, what it is, and who is in it. */
export interface GroupForm {
  name: string;
  description?: string;
  members: Member[];
}

/** The form as the manifest the API stores: an empty description is left out, not written blank. */
export function toGroupEnvelope(form: GroupForm, stored?: unknown): unknown {
  const description = form.description?.trim();
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Group",
    // What the form has no field for travels on from the manifest the edit started from (T-2470).
    metadata: { ...storedMetadata(stored), name: form.name, namespace: ORG_NAMESPACE },
    spec: {
      ...(description ? { description } : {}),
      members: (form.members ?? []).filter((member) => (member.user ?? "").trim() !== ""),
    },
  };
}

/** The stored manifest back as the form, so the fields and the YAML view hold the same group. */
export function fromGroupEnvelope(manifest: unknown): GroupForm {
  const envelope = (manifest ?? {}) as {
    metadata?: { name?: string };
    spec?: { description?: string; members?: Member[] };
  };
  return {
    name: envelope.metadata?.name ?? "",
    description: envelope.spec?.description ?? "",
    members: envelope.spec?.members ?? [],
  };
}

/** The organization's groups, which a binding or an application role may name (PF-64). */
export function useGroups() {
  return useQuery({
    queryKey: queryKeys.list(ORG_NAMESPACE, "groups"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project: ORG_NAMESPACE, plural: "groups" } },
        }),
      ),
  });
}

/**
 * A group of the organization, proposed as a change (PF-62, PF-52): who is in a group is who a
 * binding names, so a membership is reviewed in the red lane like the binding itself.
 *
 * Adding a colleague used to mean typing YAML — the right indentation under `members:` and the
 * right `- user:` key — into one textarea, for the kind an administrator touches most (T-2400).
 * It is the form every other kind has now, with the YAML view beside it for whoever prefers it.
 */
export function NewGroupDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const formRoute = useFormRoute();
  const queryClient = useQueryClient();
  const [form, setForm] = useState<GroupForm | undefined>(undefined);
  const [change, setChange] = useState<Change | null>(null);

  const propose = useMutation({
    mutationFn: async (group: GroupForm) =>
      proposeChecked(
        ORG_NAMESPACE,
        "groups",
        toGroupEnvelope(group) as ResourceProposal,
        true,
      ),
    onSuccess: (result) => {
      if (isChange(result)) {
        // Routed, the save goes back to the list and the change is shown there (T-2474).
        if (formRoute) {
          formRoute.leave(<ChangeNotice change={result} project={ORG_NAMESPACE} />);
          close(false);
        } else {
          setChange(result);
        }
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(ORG_NAMESPACE, "groups") });
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(ORG_NAMESPACE) });
    },
  });

  const failure =
    propose.error instanceof ApiError
      ? (propose.error.problem?.detail ?? propose.error.message)
      : propose.error
        ? t("app.error.generic")
        : null;

  const close = (next: boolean) => {
    if (!next) {
      setForm(undefined);
      setChange(null);
      propose.reset();
    }
    onOpenChange(next);
  };

  if (change) {
    return (
      <FormFrame
        open={open}
        onOpenChange={close}
        size="lg"
        title={t("access.groups.newTitle")}
        description={t("access.groups.newLead")}
        closeLabel={t("resourceDelete.close")}
        footer={<Button onClick={() => close(false)}>{t("resourceDelete.close")}</Button>}
      >
        <ChangeNotice change={change} project={ORG_NAMESPACE} />
      </FormFrame>
    );
  }

  return (
    <ResourceFormDialog<GroupForm>
      kind="Group"
      open={open}
      onOpenChange={close}
      title={t("access.groups.newTitle")}
      description={t("access.groups.newLead")}
      schema={groupSchema(t)}
      formData={form}
      onChange={setForm}
      project={ORG_NAMESPACE}
      draftKind="Group"
      plural="groups"
      source={{
        toManifest: (group) => toGroupEnvelope(group),
        fromManifest: (manifest) => fromGroupEnvelope(manifest),
      }}
      submitLabel={t("access.groups.propose")}
      submitting={propose.isPending}
      error={failure}
      onSubmit={(group) => propose.mutate(group)}
    />
  );
}

/**
 * Project → Access → the groups of the organization: who is in each one, and what the last
 * reconcile had to correct in Keycloak (PF-62, PF-63). Adding a member is editing the manifest,
 * which is a change somebody approves — never a direct write.
 */
export function Groups({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const groups = useGroups();
  const [writing, setWriting] = useCreateForm();
  // The same fields the new-group dialog shows, so a member is added where the group is read.
  const memberSchema = groupSchema(t);

  const rows: Row[] = asManifests(groups.data?.items ?? []).map((group: Manifest) => {
    const spec = (group.spec ?? {}) as { description?: string; members?: Member[] };
    const condition = (group.status?.conditions ?? []).find((c) => c.type === "GroupSynced");
    return {
      name: group.metadata.name,
      description: spec.description ?? "",
      members: (spec.members ?? []).map((member) => member.user ?? "").filter(Boolean),
      sync: condition
        ? {
            reason: condition.reason ?? "",
            message: condition.message ?? "",
            ok: condition.status === "True",
          }
        : undefined,
    };
  });
  const names = usePersonNames(rows.flatMap((row) => row.members));

  return (
    <section className="space-y-4" aria-labelledby="groups-heading">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 id="groups-heading" className="text-title font-semibold text-fg">
            {t("access.groups.title")}
          </h2>
          <p className="text-body text-fg-muted">{t("access.groups.lead")}</p>
        </div>
        {/* The organization is where a group lives, so the right to propose one is read there. */}
        <PermissionGuard project={ORG_NAMESPACE} kind="Group" verb="propose">
          {/* One primary per view: the Access page's own action is "Grant a role" (T-1731). */}
          <Button variant="secondary" onClick={() => setWriting(true)}>
            {t("access.groups.new")}
          </Button>
        </PermissionGuard>
      </div>

      {groups.error ? (
        <PageFailed
          error={groups.error}
          onRetry={() => {
            void groups.refetch();
          }}
        />
      ) : (
        <Table
          data-records=""
          caption={t("access.groups.caption")}
          status={groups.isPending ? t("app.loading") : undefined}
        >
          <TableHead>
            <TableHeaderCell>{t("access.groups.group")}</TableHeaderCell>
            <TableHeaderCell>{t("access.groups.members")}</TableHeaderCell>
            <TableHeaderCell>{t("access.groups.sync")}</TableHeaderCell>
            <TableHeaderCell align="right">{t("approvals.actions")}</TableHeaderCell>
          </TableHead>
          {groups.isPending ? (
            <TableSkeleton columns={4} />
          ) : (
            <TableBody>
              {rows.length === 0 ? (
                <TableEmpty columns={4}>
                  <EmptyState bare
                    title={t("access.groups.empty")}
                    description={t("access.groups.emptyHint")} />
                </TableEmpty>
              ) : (
                rows.map((row) => (
                  <TableRow key={row.name}>
                    <TableCell primary>
                      <Link
                        data-row-link=""
                        to="/organization/$tab/$"
                        params={{ tab: "groups", _splat: encodeURIComponent(row.name) }}
                        className="underline"
                      >
                        {row.name}
                      </Link>
                      {row.description ? (
                        <div className="mt-0.5 text-caption text-fg-subtle">{row.description}</div>
                      ) : null}
                    </TableCell>
                    <TableCell>
                      {row.members.length === 0 ? (
                        <span className="text-fg-subtle">{t("access.groups.nobody")}</span>
                      ) : (
                        <ul className="flex flex-col gap-0.5">
                          {row.members.map((member) => (
                            <li key={member} className="text-caption">
                              {personLabel(member, names)}
                            </li>
                          ))}
                        </ul>
                      )}
                    </TableCell>
                    <TableCell>
                      {row.sync ? (
                        <div className="flex flex-col gap-1">
                          <Badge tone={row.sync.ok ? "warning" : "danger"}>
                            {t(`access.groups.reason.${row.sync.reason}`, {
                              defaultValue: row.sync.reason,
                            })}
                          </Badge>
                          <span className="text-caption text-fg-muted">{row.sync.message}</span>
                        </div>
                      ) : (
                        <span className="text-fg-subtle">{t("access.groups.inStep")}</span>
                      )}
                    </TableCell>
                    <TableCell align="right">
                      <span className="inline-flex items-center gap-1.5">
                        {/* Adding or removing a member is editing the manifest (PF-52). */}
                        <EditResourceAction
                          target={{
                            project,
                            home: ORG_NAMESPACE,
                            kind: "Group",
                            plural: "groups",
                            name: row.name,
                            label: row.name,
                          }}
                          form={{
                            schema: memberSchema,
                            fromManifest: (manifest) =>
                              fromGroupEnvelope(manifest) as unknown as Record<string, unknown>,
                            toManifest: (edited, stored) =>
                              toGroupEnvelope(edited as unknown as GroupForm, stored),
                          }}
                        />
                        <DeleteResourceAction
                          target={{
                            project,
                            home: ORG_NAMESPACE,
                            kind: "Group",
                            plural: "groups",
                            name: row.name,
                            label: row.name,
                          }}
                        />
                      </span>
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          )}
        </Table>
      )}

      <NewGroupDialog open={writing} onOpenChange={setWriting} />
    </section>
  );
}
