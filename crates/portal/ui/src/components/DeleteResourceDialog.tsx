import { useId, useState } from "react";
import { PermissionGuard } from "./ui/PermissionGuard";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { isChange } from "../api/manifest";
import type { Change } from "../api/manifest";
import { usePermissions } from "../api/permissions";
import { ChangeNotice } from "./ChangeNotice";
import { Alert, Button, Dialog, Field, Input } from "./ui";

export interface ResourceTarget {
  project: string;
  /** The kind as the permissions name it, e.g. `Pipeline`. */
  kind: string;
  /** The plural of `/api/v1/projects/{project}/{plural}`. */
  plural: string;
  name: string;
  /** What the person calls it; the name when there is no title. */
  label?: string;
  /** Where its routes live when that is not the page's project: `org` for a Role or a RoleBinding. */
  home?: string;
}

/**
 * Removing a resource (AG-77, CC-19, CC-39): the name typed back, then one `DELETE` that opens
 * a Red change for an approver. Nothing is removed here; the answer is the change, or the
 * resources that still reference this one.
 */
export function DeleteResourceDialog({
  target,
  open,
  onOpenChange,
}: {
  target: ResourceTarget;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const inputId = useId();
  const [typed, setTyped] = useState("");
  const [change, setChange] = useState<Change | null>(null);
  const { project, plural, name } = target;
  const home = target.home ?? project;

  // What leaves with it, asked before the name is typed (T-3247): the same `DELETE` as a dry run,
  // which writes nothing. A refusal here (still referenced) is said now rather than after typing.
  const preview = useQuery({
    queryKey: [...queryKeys.list(home, plural), name, "removal"] as const,
    enabled: open,
    retry: false,
    queryFn: async () =>
      unwrap(
        await api.DELETE("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project: home, plural, name }, query: { dryRun: "All" } },
        }),
      ),
  });
  const goesWith =
    preview.data && !isChange(preview.data) && "goesWith" in preview.data ? (preview.data.goesWith ?? []) : [];

  const remove = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.DELETE("/api/v1/projects/{project}/{plural}/{name}", {
          // The name typed back: an administrator's removal is approved with it (PF-58, CC-39).
          params: { path: { project: home, plural, name }, query: { confirm: typed } },
        }),
      ),
    onSuccess: (result) => {
      if (isChange(result)) {
        setChange(result);
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project) });
    },
  });

  // Closing forgets what was typed and answered, so the next opening starts clean.
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
  const blocked = preview.error instanceof ApiError && preview.error.status === 409 ? preview.error : null;
  // The dry run answers the reference check, so only its refusal is "still in use". A 409 from the
  // removal itself after a clean dry run is another one (a removal already open, CC-34): its detail
  // says which, and calling it "still in use" sent people looking for references (T-5066).
  const referenced = blocked !== null;

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      size="sm"
      title={t("resourceDelete.title", { name: target.label ?? name })}
      description={t("resourceDelete.lead")}
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
              disabled={typed !== name}
              // Refused, not removed: the one control this dialog exists for stays in the tab
              // order and says why it will not fire, because a person who cannot see that it is
              // greyed out otherwise never learns the button is there (UI-44, T-1743).
              disabledReason={typed !== name ? t("resourceDelete.needName", { name }) : undefined}
              onClick={() => remove.mutate()}
            >
              {t("resourceDelete.propose")}
            </Button>
          </>
        )
      }
    >
      {change ? (
        <ChangeNotice change={change} project={project} />
      ) : (
        <div className="flex flex-col gap-4">
          {goesWith.length > 0 ? (
            <section aria-labelledby={`${inputId}-goes`} className="flex flex-col gap-1">
              <h3 id={`${inputId}-goes`} className="text-body font-semibold text-fg">
                {t("resourceDelete.goesWith.title")}
              </h3>
              <ul className="list-disc pl-5 text-body text-fg">
                {goesWith.map((consequence) => (
                  <li key={consequence.what}>
                    {t(`resourceDelete.goesWith.${consequence.what}`, {
                      count: consequence.count,
                      defaultValue: `${consequence.what}: ${consequence.count}`,
                    })}
                    {consequence.names && consequence.names.length > 0 ? (
                      <span className="block font-mono text-caption text-fg-muted">{consequence.names.join(", ")}</span>
                    ) : null}
                  </li>
                ))}
              </ul>
            </section>
          ) : null}
          {/* What comes back and what does not, before anyone types the name (T-3247). */}
          <p className="text-body text-fg-muted">{t("resourceDelete.restorable")}</p>
          <Field
            id={inputId}
            label={t("resourceDelete.typeName", { name })}
            help={t("resourceDelete.exact")}
            // Only once something has been typed: an empty box on opening is where the person
            // is, not a mistake they made.
            errors={typed !== "" && typed !== name ? [t("resourceDelete.mismatch", { name })] : undefined}
          >
            <Input
              id={inputId}
              value={typed}
              // Focus is the Dialog's: it puts the caret in the first field of a form it opens
              // (T-1493), so the attribute that used to sit here is one mechanism too many.
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => setTyped(event.target.value)}
            />
          </Field>
          {failure || blocked ? (
            <Alert tone="danger" role="alert" title={referenced ? t("resourceDelete.referenced") : undefined}>
              {failure ?? blocked?.problem?.detail ?? blocked?.message}
            </Alert>
          ) : null}
        </div>
      )}
    </Dialog>
  );
}

/**
 * The Delete action of one row: shown only to a person whose role may delete the kind, the
 * dialog opened on click, or at once when the page was opened with `?delete=<name>`.
 */
export function DeleteResourceAction({
  target,
  open: openedByRow,
  onOpenChange,
  trigger = true,
  label,
}: {
  target: ResourceTarget;
  /**
   * The button's words where a bare "Remove" would match another on the page (T-6423). Its accessible
   * name adds the target: the walker excuses a bare "Remove group", the grid filter's draft control.
   */
  label?: string;
  /** The row holds the state when the action lives in its menu (T-2279). */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  trigger?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const mayDelete = usePermissions(target.home ?? target.project).can(target.kind, "delete");
  const [ownOpen, setOwnOpen] = useState(
    () =>
      typeof window !== "undefined" &&
      new URLSearchParams(window.location.search).get("delete") === target.name,
  );
  // The row may open this, and so may the URL (`?edit=`/`?delete=`) or the assistant's hand-off: both
  // are honoured, and closing clears both, so a page opened on one resource still opens its dialog
  // when the row owns the trigger (T-2287).
  const open = ownOpen || (openedByRow ?? false);
  const setOpen = (next: boolean) => {
    setOwnOpen(next);
    onOpenChange?.(next);
  };
  return (
    <>
      {trigger ? (
      <PermissionGuard project={target.home ?? target.project} kind={target.kind} verb="delete">
        <Button
          size="sm"
          aria-label={label ? `${label}: ${target.label ?? target.name}` : t("resourceDelete.action", { name: target.label ?? target.name })}
          onClick={() => setOpen(true)}
        >
          {label ?? t("resourceDelete.button")}
        </Button>
      </PermissionGuard>
      ) : null}
      {mayDelete ? <DeleteResourceDialog target={target} open={open} onOpenChange={setOpen} /> : null}
    </>
  );
}
