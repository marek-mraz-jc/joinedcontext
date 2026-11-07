import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { projectDetailQuery, useProjectOrganization } from "../api/projects";
import type { Change } from "../api/manifest";
import { ChangeNotice } from "./ChangeNotice";
import { Alert, Button, Dialog, Field, Input } from "./ui";
import { PermissionGuard } from "./ui/PermissionGuard";

/** A deleted project's name stays reserved this long when the organization sets nothing (PF-78). */
const DEFAULT_COOLDOWN_DAYS = 30;

/** What the deletion of `project` carries, counted from the project's own usage (PF-77). */
function useCascade(project: string, enabled: boolean) {
  const detail = useQuery({ ...projectDetailQuery(project), enabled });
  // The project's own Organization: with several, the first of the list states another's cooldown.
  const { organization } = useProjectOrganization(project);
  const usage = ((detail.data?.status as { usage?: Record<string, number> } | undefined)?.usage ?? {}) as Record<
    string,
    number
  >;
  const policy = (organization?.spec as
    | { projects?: { nameCooldownDays?: number } }
    | undefined)?.projects;
  return {
    detail,
    counts: Object.entries(usage).filter(([, count]) => typeof count === "number" && count > 0),
    cooldown: policy?.nameCooldownDays ?? DEFAULT_COOLDOWN_DAYS,
  };
}

/**
 * Deleting a project (PF-77, PF-78, T-2605, T-2606): what goes with it listed first, the name
 * typed back, then one `DELETE /api/v1/projects/{project}` that opens the red-lane change an
 * administrator approves. Nothing is removed here, and it is never one click.
 */
export function DeleteProjectDialog({
  project,
  open,
  onOpenChange,
}: {
  project: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const inputId = useId();
  const [typed, setTyped] = useState("");
  const [change, setChange] = useState<Change | null>(null);
  const cascade = useCascade(project, open);

  const remove = useMutation({
    mutationFn: async () =>
      unwrap(await api.DELETE("/api/v1/projects/{project}", { params: { path: { project } } })),
    onSuccess: (result) => {
      setChange(result);
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project) });
    },
  });

  const close = (next: boolean) => {
    if (!next) {
      setTyped("");
      setChange(null);
      remove.reset();
    }
    onOpenChange(next);
  };

  const failure =
    remove.error instanceof ApiError
      ? (remove.error.problem?.detail ?? remove.error.message)
      : remove.error
        ? t("app.error.generic")
        : null;

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      title={t("projectDelete.title", { name: project })}
      description={t("projectDelete.lead")}
      closeLabel={t("resourceDelete.close")}
      footer={
        change ? (
          <Button onClick={() => close(false)}>{t("resourceDelete.close")}</Button>
        ) : (
          <>
            <Button variant="secondary" onClick={() => close(false)}>
              {t("form.cancel")}
            </Button>
            <Button
              variant="danger"
              loading={remove.isPending}
              disabled={typed !== project}
              disabledReason={typed !== project ? t("resourceDelete.needName", { name: project }) : undefined}
              onClick={() => remove.mutate()}
            >
              {t("projectDelete.propose")}
            </Button>
          </>
        )
      }
    >
      {change ? (
        <ChangeNotice change={change} project={project} />
      ) : (
        <div className="flex flex-col gap-4">
          <div className="space-y-2">
            <h3 className="text-body font-semibold text-fg">{t("projectDelete.cascadeTitle")}</h3>
            {cascade.detail.isPending ? (
              <p role="status" className="text-body text-fg-muted">
                {t("app.loading")}
              </p>
            ) : cascade.detail.isError ? (
              <Alert tone="danger" role="alert">
                {t("projectDelete.cascadeFailed")}
              </Alert>
            ) : (
              <ul className="list-disc space-y-1 pl-5 text-body text-fg">
                {cascade.counts.map(([dimension, count]) => (
                  <li key={dimension}>
                    {t("projectDelete.count", {
                      count: String(count),
                      what: t(`organization.field.quota.${dimension}`, { defaultValue: dimension }),
                    })}
                  </li>
                ))}
                <li>{t("projectDelete.everythingElse")}</li>
                <li>{t("projectDelete.bindings")}</li>
              </ul>
            )}
            <p className="text-body text-fg-muted">
              {t("projectDelete.cooldown", { name: project, days: String(cascade.cooldown) })}
            </p>
          </div>
          <Field
            id={inputId}
            label={t("resourceDelete.typeName", { name: project })}
            help={t("resourceDelete.exact")}
            errors={
              typed !== "" && typed !== project ? [t("resourceDelete.mismatch", { name: project })] : undefined
            }
          >
            <Input
              id={inputId}
              value={typed}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => setTyped(event.target.value)}
            />
          </Field>
          {failure ? (
            <Alert tone="danger" role="alert">
              {failure}
            </Alert>
          ) : null}
        </div>
      )}
    </Dialog>
  );
}

/** The Delete action of one project: disabled with the reason for whoever may not (UI-44). */
export function DeleteProjectAction({
  project,
  variant = "secondary",
}: {
  project: string;
  variant?: "secondary" | "danger";
}): JSX.Element {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <>
      <PermissionGuard project={project} kind="Project" verb="delete">
        <Button
          size="sm"
          variant={variant}
          aria-label={t("projectDelete.action", { name: project })}
          onClick={() => setOpen(true)}
        >
          {t("projectDelete.button")}
        </Button>
      </PermissionGuard>
      {open ? <DeleteProjectDialog project={project} open={open} onOpenChange={setOpen} /> : null}
    </>
  );
}
