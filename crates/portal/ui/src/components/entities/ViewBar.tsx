/**
 * The saved views of one type in a space (API/01 §30, T-3104): pick one, save what the grid shows
 * into it or into a new one, rename, duplicate or delete it. A view only says how the rows are
 * looked at; the rows are always read with the person's own session.
 */
import { useId, useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError } from "../../api/client";
import { createView, dataViewsKey, deleteView, listViews, sameConfig, updateView, VIEW_MODES } from "../../api/dataViews";
import type { DataView, ViewConfig, ViewKind, ViewMode } from "../../api/dataViews";
import { useIdentity } from "../../auth/AuthProvider";
import { usePermissions } from "../../api/permissions";
import { Alert, Button, ConfirmDialog, Dialog, Field, Input, RadioGroup, Select } from "../ui";

export interface ViewBarProps {
  project: string;
  space: string;
  type: string;
  /** How the rows are shown now (grid, gallery, board, …): what a view saves as its kind. */
  kind: ViewKind;
  /** The view applied now, or none for the grid as it opens. */
  selected: DataView | null;
  onSelect: (view: DataView | null) => void;
  /** What the grid shows now, as a view would save it. */
  current: ViewConfig;
  /** The grid filters on something a view does not keep (the identifier): said, never dropped silently. */
  unsaved?: string;
}

type Editing = { action: "create" | "settings"; title: string; mode: ViewMode } | null;

export function ViewBar({ project, space, type, kind, selected, onSelect, current, unsaved }: ViewBarProps): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const client = useQueryClient();
  const identity = useIdentity();
  const { can } = usePermissions(project);
  const [editing, setEditing] = useState<Editing>(null);
  const [deleting, setDeleting] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const views = useQuery({
    queryKey: dataViewsKey(project, space),
    retry: false,
    queryFn: () => listViews(project, space),
  });
  const ofType = (views.data ?? []).filter((view) => view.type === type);

  const owner = selected !== null && identity !== null && selected.owner === identity.username;
  // The server decides; this only keeps a button from offering what it would refuse.
  const steward = can("ContextSpace", "update");
  const governs = selected !== null && (owner || steward);
  const changes = selected !== null && (selected.mode !== "locked" || governs);
  const dirty = selected !== null && (selected.kind !== kind || !sameConfig(selected.config, current));

  const refresh = () => client.invalidateQueries({ queryKey: dataViewsKey(project, space) });
  const failed = (error: unknown) => {
    if (error instanceof ApiError && error.status === 409) {
      setProblem(t("spaces.saved.conflict", { detail: error.message }));
    } else {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  const save = useMutation({
    mutationFn: async (next: { title: string; mode: ViewMode; config: ViewConfig; asNew: boolean }) =>
      next.asNew || selected === null
        ? createView(project, space, { type, kind, mode: next.mode, title: next.title, config: next.config })
        : updateView(project, space, selected.id, {
            kind,
            mode: next.mode,
            title: next.title,
            config: next.config,
            expectedVersion: selected.version,
          }),
    onSuccess: (view) => {
      setProblem(null);
      setEditing(null);
      onSelect(view);
      void refresh();
    },
    onError: failed,
  });

  const remove = useMutation({
    mutationFn: (view: DataView) => deleteView(project, space, view.id),
    onSuccess: () => {
      setProblem(null);
      setDeleting(false);
      onSelect(null);
      void refresh();
    },
    onError: (error) => {
      setDeleting(false);
      failed(error);
    },
  });

  const modeLabel = (mode: string) => t(`spaces.saved.modes.${mode}`);

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-end gap-2">
        <Field id={`${id}-view`} label={t("spaces.saved.view")} className="w-full sm:w-auto">
          <Select
            id={`${id}-view`}
            value={selected?.id ?? ""}
            onChange={(e) => {
              setProblem(null);
              onSelect(ofType.find((view) => view.id === e.target.value) ?? null);
            }}
          >
            <option value="">{t("spaces.inside.allRows")}</option>
            {ofType.map((view) => (
              <option key={view.id} value={view.id}>
                {`${view.title} · ${modeLabel(view.mode)}`}
              </option>
            ))}
          </Select>
        </Field>
        {selected !== null ? (
          <Button
            variant="primary"
            disabled={!dirty || !changes || save.isPending}
            disabledReason={!changes ? t("spaces.saved.lockedReason") : undefined}
            onClick={() => save.mutate({ title: selected.title, mode: selected.mode as ViewMode, config: current, asNew: false })}
          >
            {t("spaces.saved.save")}
          </Button>
        ) : null}
        <Button onClick={() => setEditing({ action: "create", title: selected ? t("spaces.saved.copyOf", { title: selected.title }) : "", mode: "personal" })}>
          {selected ? t("spaces.saved.saveAs") : t("spaces.saved.saveNew")}
        </Button>
        {selected !== null ? (
          <>
            <Button
              disabled={!changes}
              disabledReason={!changes ? t("spaces.saved.lockedReason") : undefined}
              onClick={() => setEditing({ action: "settings", title: selected.title, mode: selected.mode as ViewMode })}
            >
              {t("spaces.saved.settings")}
            </Button>
            <Button
              variant="danger"
              disabled={!governs}
              disabledReason={!governs ? t("spaces.saved.governReason") : undefined}
              onClick={() => setDeleting(true)}
            >
              {t("spaces.saved.delete")}
            </Button>
          </>
        ) : null}
      </div>
      {views.isError ? <p className="text-body text-fg-muted">{t("spaces.saved.unavailable")}</p> : null}
      {selected !== null && dirty ? <p className="text-caption text-fg-muted">{t("spaces.saved.dirty")}</p> : null}
      {unsaved ? <p className="text-caption text-fg-muted">{unsaved}</p> : null}
      {problem ? (
        <Alert tone="danger" title={t("spaces.saved.failed")}>
          <p>{problem}</p>
          <Button size="sm" onClick={() => void refresh().then(() => setProblem(null))}>
            {t("spaces.saved.reload")}
          </Button>
        </Alert>
      ) : null}

      <Dialog
        open={editing !== null}
        onOpenChange={(open) => !open && setEditing(null)}
        title={editing?.action === "settings" ? t("spaces.saved.settingsTitle") : t("spaces.saved.createTitle", { type })}
        closeLabel={t("app.close")}
        footer={
          <>
            <Button onClick={() => setEditing(null)}>{t("app.cancel")}</Button>
            <Button
              variant="primary"
              disabled={!editing || editing.title.trim() === "" || editing.title.length > 120 || save.isPending}
                disabledReason={save.isPending ? t("app.busyReason") : t("spaces.saved.titleFirst")}
              onClick={() =>
                editing &&
                save.mutate({
                  title: editing.title.trim(),
                  mode: editing.mode,
                  // Settings keep what the view saved; a new view takes what the grid shows.
                  config: editing.action === "settings" && selected ? selected.config : current,
                  asNew: editing.action === "create",
                })
              }
            >
              {t("spaces.saved.confirm")}
            </Button>
          </>
        }
      >
        {editing ? (
          <div className="flex flex-col gap-3">
            <Field id={`${id}-title`} label={t("spaces.saved.title")} required help={t("spaces.saved.titleHelp")}>
              <Input
                id={`${id}-title`}
                value={editing.title}
                maxLength={120}
                onChange={(e) => setEditing({ ...editing, title: e.target.value })}
              />
            </Field>
            <RadioGroup<ViewMode>
              name={`${id}-mode`}
              legend={t("spaces.saved.mode")}
              value={editing.mode}
              onChange={(mode) => setEditing({ ...editing, mode })}
              options={VIEW_MODES.map((mode) => ({
                value: mode,
                label: modeLabel(mode),
                // Who sees a view is its owner's or a steward's to change.
                disabled: editing.action === "settings" && !governs && mode !== selected?.mode,
                description:
                  editing.action === "settings" && !governs && mode !== selected?.mode
                    ? t("spaces.saved.modeLocked")
                    : t(`spaces.saved.modeHelp.${mode}`),
              }))}
            />
          </div>
        ) : null}
      </Dialog>

      <ConfirmDialog
        open={deleting}
        onOpenChange={setDeleting}
        title={t("spaces.saved.deleteTitle", { title: selected?.title ?? "" })}
        description={t("spaces.saved.deleteHelp")}
        confirmLabel={t("spaces.saved.delete")}
        pending={remove.isPending}
        onConfirm={() => selected && remove.mutate(selected)}
      />
    </div>
  );
}
