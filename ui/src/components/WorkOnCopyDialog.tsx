import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { components } from "../api/schema";
import { api, ApiError, unwrap } from "../api/client";
import { usePermissions } from "../api/permissions";
import { Alert, Button, Dialog, Field, Input, Select } from "./ui";
import { useWorkspace } from "./layout/WorkspaceContext";

export interface WorkOnCopyScope {
  kind: "project" | "space" | "resources";
  name?: string;
  items?: { kind: string; name: string }[];
}

export interface WorkOnCopyDialogProps {
  project: string;
  scope: WorkOnCopyScope;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

function defaultName(scope: WorkOnCopyScope): string {
  let raw: string;
  if (scope.kind === "space" && scope.name) {
    raw = scope.name;
  } else if (scope.kind === "resources" && scope.items?.length === 1) {
    raw = scope.items[0].name;
  } else {
    raw = "copy";
  }
  // Lowercase, replace non-alphanumeric/dash with dash, cut to 20.
  return raw
    .toLowerCase()
    .replace(/[^a-z0-9-]/g, "-")
    .slice(0, 20);
}

function isValidName(name: string): boolean {
  return /^[a-z0-9]([a-z0-9-]{0,18}[a-z0-9])?$/.test(name) && name.length <= 20;
}

export function WorkOnCopyDialog({
  project,
  scope,
  open,
  onOpenChange,
}: WorkOnCopyDialogProps): JSX.Element {
  const { t } = useTranslation();
  const { enter } = useWorkspace();
  const [name, setName] = useState(() => defaultName(scope));
  const [title, setTitle] = useState("");
  const [ttlDays, setTtlDays] = useState(7);
  const [error, setError] = useState<string | null>(null);
  // A name another copy holds is answered at the name, where it is fixed (T-3219).
  const [taken, setTaken] = useState<string | null>(null);

  const scopeText = useMemo(() => {
    if (scope.kind === "project") return t("workspaces.open.scopeProject");
    if (scope.kind === "space") return t("workspaces.open.scopeSpace", { name: scope.name });
    if (scope.kind === "resources" && scope.items) {
      return scope.items.map((i) => `${i.kind}/${i.name}`).join(", ");
    }
    return "";
  }, [scope, t]);

  const create = useMutation({
    mutationFn: async () => {
      const body: components["schemas"]["OpenRequest"] = {
        name,
        title: title.trim() || undefined,
        ttlDays,
        scope:
          scope.kind === "space"
            ? { kind: "space", name: scope.name ?? "" }
            : scope.kind === "resources"
              ? { kind: "resources", items: scope.items ?? [] }
              : { kind: "project" },
      };
      return unwrap(
        await api.POST("/api/v1/projects/{project}/workspaces", {
          params: { path: { project } },
          body,
        }),
      );
    },
    onSuccess: () => {
      enter(name);
      onOpenChange(false);
    },
    onError: (err: Error) => {
      if (err instanceof ApiError && err.status === 409) {
        setTaken(name);
        return;
      }
      setError(err instanceof ApiError ? (err.problem?.detail ?? err.message) : err.message);
    },
  });

  const canSubmit = isValidName(name) && taken !== name;

  // Closing forgets what was typed, whichever way it is closed. Cancel used to call
  // `onOpenChange(false)` straight past this, so only the Escape key and the X reset the dialog
  // (T-1755).
  const close = (next: boolean) => {
    if (!next) {
      setName(defaultName(scope));
      setTitle("");
      setTtlDays(7);
      setError(null);
      setTaken(null);
    }
    onOpenChange(next);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      size="md"
      title={t("workspaces.open.title")}
      description={t("workspaces.open.description")}
      // Not "Cancel": the footer already has a Cancel, and two controls in one dialog answering
      // to the same name is a dialog nobody can drive by voice or by a screen reader's list.
      closeLabel={t("app.close")}
      footer={
        <>
          <Button variant="secondary" onClick={() => close(false)}>
            {t("form.cancel")}
          </Button>
          <Button
            variant="primary"
            disabled={!canSubmit}
            // Reachable while it is refused, saying what the name has to be (UI-44).
            disabledReason={
              canSubmit ? undefined : taken === name ? t("workspaces.open.nameTaken", { name }) : t("workspaces.open.nameInvalid")
            }
            loading={create.isPending}
            onClick={() => create.mutate()}
          >
            {t("workspaces.open.submit")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        {error ? (
          <Alert tone="danger" role="alert">
            {error}
          </Alert>
        ) : null}
        {/* The hint and the refusal are the Field's, so the Input goes `aria-invalid` and both
            are in its `aria-describedby`. They used to be loose paragraphs beside it: somebody
            returning to fix the name heard the label and nothing about what was wrong (T-1755). */}
        <Field
          id="ws-name"
          label={t("workspaces.open.name")}
          required
          help={t("workspaces.open.nameHint")}
          errors={
            name !== "" && !isValidName(name)
              ? [t("workspaces.open.nameInvalid")]
              : taken === name
                ? [t("workspaces.open.nameTaken", { name })]
                : undefined
          }
        >
          <Input
            id="ws-name"
            value={name}
            onChange={(e) => {
              setName(e.target.value);
              setError(null);
            }}
          />
        </Field>
        {/* Its own label: it used to borrow `workspaces.open.title`, which is the dialog's own
            heading, so the second box was called "Work on a copy" as well. */}
        <Field
          id="ws-title"
          label={t("workspaces.open.titleField")}
          help={t("workspaces.open.titleHint")}
        >
          <Input id="ws-title" value={title} onChange={(e) => setTitle(e.target.value)} />
        </Field>
        {/* Not a Field: a Field always renders `<label htmlFor>`, and there is no control here
            for the label to point at — "Covers" was an orphan label over static text. */}
        <div className="flex flex-col gap-1.5">
          <p className="text-body font-medium text-fg">{t("workspaces.open.scope")}</p>
          <p className="text-body text-fg-muted">{scopeText}</p>
        </div>
        <Field id="ws-ttl" label={t("workspaces.open.ttl")} help={t("workspaces.open.ttlHint")}>
          <Select
            id="ws-ttl"
            value={ttlDays}
            onChange={(e) => setTtlDays(Number(e.target.value))}
          >
            {[1, 3, 7, 14].map((d) => (
              <option key={d} value={d}>
                {t("workspaces.open.ttlDays", { days: d })}
              </option>
            ))}
          </Select>
        </Field>
      </div>
    </Dialog>
  );
}

export function WorkOnCopyAction({
  project,
  scope,
  label,
  variant = "secondary",
  open: openedByRow,
  onOpenChange,
  trigger = true,
}: {
  project: string;
  scope: WorkOnCopyScope;
  label?: string;
  variant?: "primary" | "secondary";
  /** The row holds the state when the action lives in its menu (T-2279). */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  trigger?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  // Opening a copy proposes into the project, and the API refuses it without a role that may
  // propose something there (`ops/workspaces.rs`, `may_propose_anything`). The refusal used to
  // arrive after the work, as a raw server sentence in the dialog's Alert (T-1755).
  const mayOpen = usePermissions(project).can("*", "propose");
  const [ownOpen, setOwnOpen] = useState(false);
  // The row may open this, and so may the URL (`?edit=`/`?delete=`) or the assistant's hand-off: both
  // are honoured, and closing clears both, so a page opened on one resource still opens its dialog
  // when the row owns the trigger (T-2287).
  // An address that asks for the dialog (`/workspaces/new`) opens it only for a role that may
  // use it; anyone else sees the list with the disabled button and its reason (T-2749).
  const open = ownOpen || ((openedByRow ?? false) && mayOpen);
  const setOpen = (next: boolean) => {
    setOwnOpen(next);
    onOpenChange?.(next);
  };
  return (
    <>
      {trigger ? (
        <Button
          variant={variant}
          size="sm"
          disabled={!mayOpen}
          disabledReason={mayOpen ? undefined : t("workspaces.open.denied")}
          onClick={() => setOpen(true)}
        >
          {label ?? t("workspaces.open.action")}
        </Button>
      ) : null}
      {open ? (
        <WorkOnCopyDialog
          project={project}
          scope={scope}
          open={open}
          onOpenChange={setOpen}
        />
      ) : null}
    </>
  );
}
