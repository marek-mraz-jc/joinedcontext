import { useState } from "react";
import type { ComponentProps, JSX, ReactNode } from "react";
import { PermissionGuard } from "./ui/PermissionGuard";
import { useTranslation } from "react-i18next";
import { usePermissions } from "../api/permissions";
import { takeEditRequest } from "../assistant/state";
import type { JsonSchema, UiSchema } from "./forms/types";
import type { ResourceTarget } from "./DeleteResourceDialog";
import { Button } from "./ui";
import { useEditForm } from "./forms/FormRoute";
import type { EditResourceDialogBody } from "./EditResourceDialogBody";
import { useLoadedWhenOpen } from "./loadWhenOpen";

// The form engine, its widgets and the YAML editor load when a dialog opens, not with every list
// page that offers Edit (T-3316).
const body: { current?: typeof EditResourceDialogBody } = {};
const load = () => import("./EditResourceDialogBody").then((module) => module.EditResourceDialogBody);

/**
 * The kind's own form, for editing a resource whose page already has one (T-2278, UI-61).
 *
 * Without this the Edit action opened the manifest as YAML for every kind, including the ones whose
 * create dialog has a form — so a steward who filled in fields to make a context space was handed a
 * text editor to change it, and what they saw first was Monaco's line numbers. The two directions are
 * the same pair the create form uses, so a page passes what it already built.
 */
export interface EditableForm {
  schema: JsonSchema;
  uiSchema?: UiSchema;
  /** The stored manifest as the form's own model. */
  fromManifest: (manifest: unknown) => Record<string, unknown>;
  /**
   * The form's model back as the manifest to propose. `stored` is the manifest the edit started
   * from: whatever the form has no field for (a title, a description, a label) is taken from it,
   * or the edit would propose deleting it (T-2470).
   */
  toManifest: (form: Record<string, unknown>, stored: unknown) => unknown;
  /** Panels under the fields that follow the manifest as it is edited, e.g. a live preview (T-4693). */
  afterFields?: (manifest: unknown) => ReactNode;
  /**
   * The stored record read in plain words above its fields (T-5591). With one, the fields fold
   * under "Show the manifest fields": closed for a reader, open for a person who may change them.
   */
  summary?: (manifest: unknown) => ReactNode;
}

/**
 * Editing a resource: the kind's form when the page gave one, else its manifest as YAML (AG-77,
 * CC-19), then one `PUT` that opens a change for an approver. Nothing of it loads until it opens.
 */
export function EditResourceDialog(props: ComponentProps<typeof EditResourceDialogBody>): JSX.Element | null {
  const ready = useLoadedWhenOpen(props.open, body, load);
  return props.open && ready ? <Loaded {...props} /> : null;
}

function Loaded(props: ComponentProps<typeof EditResourceDialogBody>): JSX.Element | null {
  const Dialog = body.current;
  return Dialog ? <Dialog {...props} /> : null;
}

/**
 * The Edit action of one row: its button refused with the reason to a person whose role may not
 * propose the kind, the dialog opened on click, from the row's link (`…/{name}/edit`), or at once
 * when the page was opened with `?edit=<name>`, on the change the assistant made when it made one.
 * Opened by a person who may not propose, the dialog shows the record read only (T-2875).
 */
export function EditResourceAction({
  target,
  form,
  open: openedByRow,
  onOpenChange,
  trigger = true,
  addressed = false,
}: {
  target: ResourceTarget;
  /** The kind's own form, when its page has one to give (T-2278). */
  form?: EditableForm;
  /**
   * The row holds the state instead, because a dialog opened from a menu cannot live inside it: the
   * menu unmounts when it closes and would take the dialog with it (T-2279). With `trigger={false}`
   * this renders the dialog alone and the row's menu item opens it.
   */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  trigger?: boolean;
  /**
   * The page hosts its forms at an address (`…/{name}/edit`) and this row is the one the address
   * names: the address opens the dialog, and the Edit button goes to it (T-2875). Left off where a
   * name could be another row's, e.g. a shared reference on the endpoints page.
   */
  addressed?: boolean;
}): JSX.Element {
  const { t } = useTranslation();
  const routed = useEditForm(target.name);
  const byAddress = addressed && routed !== null ? routed : null;
  const mayPropose = usePermissions(target.home ?? target.project).can(target.kind, "propose");
  const [request] = useState(() => takeEditRequest(target.name));
  const [ownOpen, setOwnOpen] = useState(request !== null);
  // The row may open this, and so may the URL (`?edit=`/`?delete=`) or the assistant's hand-off: both
  // are honoured, and closing clears both, so a page opened on one resource still opens its dialog
  // when the row owns the trigger (T-2287).
  const open = ownOpen || (byAddress?.[0] ?? false) || (openedByRow ?? false);
  const setOpen = (next: boolean) => {
    setOwnOpen(next && byAddress === null);
    if (byAddress && next !== byAddress[0]) {
      byAddress[1](next);
    }
    onOpenChange?.(next);
  };
  return (
    <>
      {trigger ? (
      <PermissionGuard project={target.home ?? target.project} kind={target.kind} verb="propose">
        <Button
          size="sm"
          aria-label={t("resourceEdit.action", { name: target.label ?? target.name })}
          onClick={() => setOpen(true)}
        >
          {t("resourceEdit.button")}
        </Button>
      </PermissionGuard>
      ) : null}
      {/* A person who may not propose still opens the record, read only (T-2875). */}
      <EditResourceDialog
        target={target}
        open={open}
        onOpenChange={setOpen}
        changed={mayPropose ? request?.manifest : null}
        form={form}
        readOnly={!mayPropose}
      />
    </>
  );
}
