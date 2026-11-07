import { useState } from "react";
import type { JSX, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { usePermissions } from "../api/permissions";
import { DeleteResourceAction } from "./DeleteResourceDialog";
import type { ResourceTarget } from "./DeleteResourceDialog";
import { EditResourceAction } from "./EditResourceDialog";
import type { EditableForm } from "./EditResourceDialog";
import { SaveAsResourceAction } from "./SaveAsDialog";
import { WorkOnCopyAction } from "./WorkOnCopyDialog";
import type { WorkOnCopyScope } from "./WorkOnCopyDialog";
import { RowActions } from "./ui/RowActions";
import { deniedReason } from "./ui/PermissionGuard";
import { safeHref } from "./ui/safeHref";
import { useEditForm } from "./forms/FormRoute";
import type { RowAction } from "./ui/RowActions";

/**
 * The four actions every manifest row has, behind one menu (T-2287, T-2279, UI-26, UI-44).
 *
 * Every kind's table used to paint Edit, Save as, Work on a copy and Delete side by side, so four
 * controls competed with the data in every row of every list. This is that cell, once: the page's own
 * obvious action stays in the open as `primary`, the four go in the menu, and an action the role cannot
 * take stays listed, disabled, with the sentence that names the verb and the kind.
 *
 * The dialogs are rendered beside the menu and opened by the row's own state, because Radix unmounts a
 * menu's content when it closes and would take an open dialog with it.
 */
export function ResourceRowActions({
  project,
  target,
  scope,
  form,
  onEdit,
  primary,
  extra,
  sourceUrl,
}: {
  project: string;
  target: ResourceTarget;
  /** What "Work on a copy" copies; a kind that is not a space copies itself. */
  scope?: WorkOnCopyScope;
  /** The kind's own form for editing, when its page has one; without it the manifest opens as text. */
  form?: EditableForm;
  /** A page with its own editor (a pipeline, a data source) handles Edit itself instead. */
  onEdit?: () => void;
  primary?: ReactNode;
  /**
   * The row's own actions, listed before the four in the same menu with a line after them: one
   * menu per row, never a second one beside it (T-2618).
   */
  extra?: RowAction[];
  /**
   * `status.sourceUrl`: the manifest in the forge, an item of the menu instead of an icon column
   * (T-6095). Written by the manifest's author, so only an address that navigates is listed.
   */
  sourceUrl?: string | null;
}): JSX.Element {
  const { t } = useTranslation();
  const permissions = usePermissions(target.home ?? project);
  const [openAction, setOpenAction] = useState<"edit" | "saveAs" | "copy" | "delete" | null>(null);
  // On a routed list the edit form is a page at `/{plural}/{name}/edit` (T-2474): the menu goes
  // there, and the address opens it.
  const routedEdit = useEditForm(target.name);
  const opens = (action: "edit" | "saveAs" | "copy" | "delete") => (open: boolean) =>
    setOpenAction(open ? action : null);
  const denied = (verb: "propose" | "delete") =>
    permissions.can(target.kind, verb) ? undefined : deniedReason(t, target.home ?? project, verb, target.kind);

  const source = safeHref(sourceUrl);
  const actions: RowAction[] = [
    {
      key: "edit",
      label: t("resourceEdit.button"),
      onSelect: onEdit ?? (() => (routedEdit ? routedEdit[1](true) : setOpenAction("edit"))),
      disabledReason: denied("propose"),
    },
    ...(source ? [{ key: "source", label: t("resourceList.viewSource"), href: source }] : []),
    {
      key: "saveAs",
      label: t("saveAs.button"),
      onSelect: () => setOpenAction("saveAs"),
      disabledReason: denied("propose"),
    },
    {
      key: "copy",
      label: t("workspaces.open.action"),
      onSelect: () => setOpenAction("copy"),
    },
    {
      key: "delete",
      label: t("resourceDelete.button"),
      tone: "danger",
      onSelect: () => setOpenAction("delete"),
      disabledReason: denied("delete"),
    },
  ];

  return (
    <>
      <RowActions
        label={target.label ?? target.name}
        actions={[
          ...(extra ?? []).map((action, index, all) =>
            index === all.length - 1 ? { ...action, separatorAfter: true } : action,
          ),
          ...actions,
        ]}
        primary={primary}
      />
      {onEdit ? null : (
        <EditResourceAction
          target={target}
          form={form}
          trigger={false}
          open={routedEdit ? routedEdit[0] : openAction === "edit"}
          onOpenChange={routedEdit ? routedEdit[1] : opens("edit")}
        />
      )}
      <SaveAsResourceAction
        target={target}
        trigger={false}
        open={openAction === "saveAs"}
        onOpenChange={opens("saveAs")}
      />
      <WorkOnCopyAction
        project={project}
        scope={scope ?? { kind: "resources", items: [{ kind: target.kind, name: target.name }] }}
        trigger={false}
        open={openAction === "copy"}
        onOpenChange={opens("copy")}
      />
      <DeleteResourceAction
        target={target}
        trigger={false}
        open={openAction === "delete"}
        onOpenChange={opens("delete")}
      />
    </>
  );
}
