import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import type { Change } from "../../api/manifest";
import { usePermissions } from "../../api/permissions";
import { ChangeNotice } from "../ChangeNotice";
import { Alert, Button, Dialog, Field, Icon, Input } from "../ui";

/** A DNS-1123 label, which is what a project slug is (PF-67). */
const LABEL = /^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/;

/** The organization's own namespace, which is never a project (PF-67). */
const ORG = "org";

/** What is wrong with the name the person is typing, live; `null` while it is fine. */
export function nameProblem(name: string): "empty" | "label" | "reserved" | null {
  if (name.trim() === "") {
    return "empty";
  }
  if (name === ORG) {
    return "reserved";
  }
  return LABEL.test(name) && name.length <= 63 ? null : "label";
}

/**
 * Opening a project from the sidebar (PF-65, PF-66, UI-44): the API says whether this caller may,
 * and the control is disabled with that reason rather than hidden. The result is the change a
 * person approves, or the project itself when the organization lets anyone open one.
 */
export function NewProjectDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const ids = useId();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [description, setDescription] = useState("");
  const [change, setChange] = useState<Change | null>(null);

  const problem = nameProblem(name);

  const openProject = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/projects", {
          body: {
            name,
            displayName: displayName.trim() === "" ? undefined : displayName,
            description: description.trim() === "" ? undefined : description,
          },
        }),
      ),
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects() });
      // Nobody is waiting: the project is open, so the sidebar moves to it (PF-66).
      if (result.status.phase === "Merged") {
        close(false);
        void navigate({
          to: "/projects/$project/$plural",
          params: { project: name, plural: "spaces" },
        });
        return;
      }
      setChange(result);
    },
  });

  const close = (next: boolean) => {
    if (!next) {
      setName("");
      setDisplayName("");
      setDescription("");
      setChange(null);
      openProject.reset();
    }
    onOpenChange(next);
  };

  const failure =
    openProject.error instanceof ApiError
      ? (openProject.error.problem?.detail ?? openProject.error.message)
      : openProject.error
        ? t("app.error.generic")
        : null;

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      title={t("projects.newTitle")}
      description={t("projects.newLead")}
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
              // `loading`, not `disabled`: a button that leaves the tab order while the POST is
              // in flight moves focus somewhere nobody chose, and there was no spinner and no
              // `aria-busy` to say anything was happening at all (T-1758, UI-15).
              loading={openProject.isPending}
              disabled={problem !== null}
              // Refused and reachable, saying that the name is what stands in the way (UI-44).
              // Not the field's own message repeated: the Field above already carries which of
              // the three it is, and a screen reader would otherwise read it twice.
              disabledReason={problem === null ? undefined : t("projects.nameNeeded")}
              onClick={() => openProject.mutate()}
            >
              {t("projects.open")}
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
          <Field id={`${ids}-display`} label={t("projects.displayNameLabel")} help={t("projects.displayNameHint")}>
            <Input
              id={`${ids}-display`}
              value={displayName}
              onChange={(event) => setDisplayName(event.target.value)}
            />
          </Field>
          <Field id={`${ids}-description`} label={t("projects.descriptionLabel")} help={t("projects.descriptionHint")}>
            <Input
              id={`${ids}-description`}
              value={description}
              onChange={(event) => setDescription(event.target.value)}
            />
          </Field>
        </div>
      )}
    </Dialog>
  );
}

/**
 * The sidebar's "New project" control. Who may open a project is the organization's own setting,
 * which `permissions/me` answers, and the refusal is shown on the control rather than by hiding
 * it (UI-44, PF-65). The API is what refuses; this only says so first.
 */
export function NewProjectButton({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const permissions = usePermissions(project);
  const [writing, setWriting] = useState(false);
  const creation = permissions.data?.projects?.creation;
  const allowed = creation?.allowed !== false;
  const reason = creation?.reason ?? t("projects.notAllowed");
  return (
    <>
      {/* The reason goes to the button, which makes itself `aria-disabled` and keeps its place
          in the tab order (`Button.disabledReason`). The wrapper this used to render — a bare
          `tabIndex={0}` span with no role and no name, around a hard-`disabled` button whose
          `disabled:pointer-events-none` swallowed the `title`, with a `role="tooltip"` nothing
          referenced — is the exact shape `PermissionGuard.tsx:15-19` names as the bug: a
          keyboard user landed on an unnamed stop, a mouse user got no tooltip, and nobody was
          told why (T-1758, UI-44). */}
      <Button
        variant="secondary"
        size="sm"
        className="w-full justify-center"
        disabled={!allowed}
        disabledReason={allowed ? undefined : reason}
        icon={<Icon name="plus" className="size-4" />}
        onClick={() => setWriting(true)}
      >
        {t("projects.new")}
      </Button>
      <NewProjectDialog open={writing} onOpenChange={setWriting} />
    </>
  );
}
