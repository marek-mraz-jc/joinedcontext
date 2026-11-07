import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import type { Change } from "../api/manifest";
import { ChangeNotice } from "./ChangeNotice";
import { nameProblem } from "./layout/NewProject";
import { Alert, Button, Dialog, Field, Input } from "./ui";

/**
 * Duplicating a project (PF-89, T-2643): the new slug and its title, then one
 * `POST /api/v1/projects/{project}/duplicate`. The Portal copies the repository with its history
 * and proposes the copy's registry entry; the copy's endpoints answer at slugs of their own.
 */
export function DuplicateProjectDialog({
  project,
  open,
  onOpenChange,
}: {
  project: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const ids = useId();
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [change, setChange] = useState<Change | null>(null);
  const problem = nameProblem(name) ?? (name === project ? "same" : null);

  const duplicate = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/duplicate", {
          params: { path: { project } },
          body: { name, displayName: displayName.trim() === "" ? undefined : displayName, parameters: {} },
        }),
      ),
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects() });
      setChange(result);
    },
  });

  const close = (next: boolean) => {
    if (!next) {
      setName("");
      setDisplayName("");
      setChange(null);
      duplicate.reset();
    }
    onOpenChange(next);
  };

  const failure =
    duplicate.error instanceof ApiError
      ? (duplicate.error.problem?.detail ?? duplicate.error.message)
      : duplicate.error
        ? t("app.error.generic")
        : null;

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      title={t("projectDuplicate.title", { name: project })}
      description={t("projectDuplicate.lead")}
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
              variant="primary"
              loading={duplicate.isPending}
              disabled={problem !== null}
              disabledReason={problem === null ? undefined : t("projects.nameNeeded")}
              onClick={() => duplicate.mutate()}
            >
              {t("projectDuplicate.propose")}
            </Button>
          </>
        )
      }
    >
      {change ? (
        <ChangeNotice change={change} project={name} />
      ) : (
        <div className="flex flex-col gap-4">
          {failure ? (
            <Alert tone="danger" role="alert">
              {failure}
            </Alert>
          ) : null}
          <Field
            id={`${ids}-name`}
            label={t("projects.nameLabel")}
            help={t("projects.nameHint")}
            errors={
              problem === "label"
                ? [t("projects.nameInvalid")]
                : problem === "reserved"
                  ? [t("projects.nameReserved")]
                  : problem === "same"
                    ? [t("projectDuplicate.sameName")]
                    : undefined
            }
            required
          >
            <Input
              id={`${ids}-name`}
              value={name}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => setName(event.target.value)}
            />
          </Field>
          <Field id={`${ids}-display`} label={t("projects.displayNameLabel")}>
            <Input
              id={`${ids}-display`}
              value={displayName}
              onChange={(event) => setDisplayName(event.target.value)}
            />
          </Field>
          <p className="text-caption text-fg-muted">{t("projectDuplicate.note")}</p>
        </div>
      )}
    </Dialog>
  );
}

/**
 * The project's Duplicate control. Only a project in a repository of its own can be copied;
 * any other says so on the control rather than hiding it (UI-44).
 */
export function DuplicateProjectAction({
  project,
  inOwnRepository,
}: {
  project: string;
  inOwnRepository: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button
        size="sm"
        variant="secondary"
        disabled={!inOwnRepository}
        disabledReason={inOwnRepository ? undefined : t("projectDuplicate.needsRepository")}
        onClick={() => setOpen(true)}
      >
        {t("projectDuplicate.button")}
      </Button>
      <DuplicateProjectDialog project={project} open={open} onOpenChange={setOpen} />
    </>
  );
}
