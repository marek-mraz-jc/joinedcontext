import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError } from "../../api/client";
import { door, doorAction } from "../../api/odata";
import type { RestAnswer } from "../../api/odata";
import { ManifestGrid } from "../../components/grid/ManifestGrid";
import { Alert, Badge, Button, Card, EmptyState, Field, FileDropZone, FilePicker, Input, PageFailed, PageLoading } from "../../components/ui";
import type { PortalSpec } from "./PortalsArea";

type FileRow = RestAnswer<"list_portal_files">["items"][number] & { usedBy: string[] };

/** What a refused files call says, in the server's words when it named the limit or the use. */
function refusedText(error: unknown, generic: string): string {
  if (error instanceof ApiError) return error.problem?.detail ?? generic;
  if (error instanceof Error && error.message) return error.message;
  return generic;
}

/** `1.5 MB`, `320 kB`, `12 B`. */
export function sizeText(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} kB`;
  return `${bytes} B`;
}

/** The short name of a stored type, as the grid's Type column reads it. */
export function typeText(contentType: string): string {
  const known: Record<string, string> = {
    "image/png": "PNG",
    "image/jpeg": "JPEG",
    "image/webp": "WebP",
    "image/svg+xml": "SVG",
    "application/pdf": "PDF",
    "text/csv": "CSV",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet": "XLSX",
  };
  return known[contentType] ?? contentType;
}

/** Sends `file`'s bytes to the upload (no `id`) or the replace route; the type is read from the bytes on the server. */
async function send(portal: string, file: File, id?: string): Promise<void> {
  const body = file as unknown as string;
  const options = { bodySerializer: (raw: unknown) => raw as BodyInit, headers: { "Content-Type": "application/octet-stream" } };
  const { error, response } = id
    ? await api.PUT("/api/v1/projects/org/portals/{name}/files/{id}", { params: { path: { name: portal, id } }, body, ...options })
    : await api.POST("/api/v1/projects/org/portals/{name}/files", { params: { path: { name: portal }, query: { name: file.name } }, body, ...options });
  if (!response.ok) {
    const answered = (error ?? {}) as { detail?: string; title?: string };
    throw new Error(answered.detail ?? answered.title ?? `HTTP ${response.status}`);
  }
}

/**
 * The Files tab of a portal (T-7417, T-7428): its uploaded images and documents in the shared
 * grid, with upload by drag and drop, rename, replace and delete (refused while a portal uses the
 * file). The files of the Organization's default portal are served on every one of its portals.
 */
export function PortalFilesTab({ name, spec }: { name: string; spec: PortalSpec }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const key = ["portalFiles", name];
  const [notice, setNotice] = useState<{ tone: "success" | "danger"; text: string } | null>(null);
  const [uploading, setUploading] = useState<{ done: number; total: number; name: string } | null>(null);
  const [renaming, setRenaming] = useState<{ id: string; name: string } | null>(null);
  const [deleting, setDeleting] = useState<FileRow | null>(null);
  const files = useQuery({
    queryKey: key,
    retry: false,
    queryFn: async () => {
      const answer = (await door("portal_portal_files", { name })) as RestAnswer<"list_portal_files">;
      return { ...answer, items: answer.items.map((f): FileRow => ({ ...f, usedBy: f.usedBy ?? [] })) };
    },
  });
  const refresh = () => void queryClient.invalidateQueries({ queryKey: key });
  const upload = async (picked: File[], replacing?: FileRow) => {
    setNotice(null);
    const failed: string[] = [];
    for (const [index, file] of picked.entries()) {
      setUploading({ done: index, total: picked.length, name: file.name });
      try {
        await send(name, file, replacing?.id);
      } catch (error) {
        failed.push(`${file.name}: ${refusedText(error, t("app.error.generic"))}`);
      }
    }
    setUploading(null);
    refresh();
    setNotice(
      failed.length > 0
        ? { tone: "danger", text: failed.join("; ") }
        : { tone: "success", text: replacing ? t("portals.files.replaced", { name: replacing.name }) : t("portals.files.uploaded", { count: picked.length }) },
    );
  };
  const rename = useMutation({
    mutationFn: async (next: { id: string; name: string }) => {
      await doorAction("portal_portal_file_rename", { name, id: next.id, body: { name: next.name.trim() } });
    },
    onSuccess: () => {
      setRenaming(null);
      setNotice({ tone: "success", text: t("portals.files.renamed") });
      refresh();
    },
    onError: (error) => setNotice({ tone: "danger", text: refusedText(error, t("app.error.generic")) }),
  });
  const remove = useMutation({
    mutationFn: async (file: FileRow) => {
      await doorAction("portal_portal_file_delete", { name, id: file.id });
    },
    onSuccess: (_, file) => {
      setDeleting(null);
      setNotice({ tone: "success", text: t("portals.files.deleted", { name: file.name }) });
      refresh();
    },
    onError: (error) => {
      setDeleting(null);
      setNotice({ tone: "danger", text: refusedText(error, t("app.error.generic")) });
    },
  });

  if (files.isPending) return <PageLoading label={t("app.loading")} />;
  if (files.isError) {
    return files.error instanceof ApiError && files.error.status === 503 ? (
      <EmptyState icon="globe" title={t("portals.files.noStoreTitle")} description={t("portals.files.noStore")} />
    ) : (
      <PageFailed error={files.error} onRetry={() => void files.refetch()} />
    );
  }
  const { items, limits } = files.data;
  const full = limits.files >= limits.maxFiles || limits.bytes >= limits.maxBytes;
  const scope = spec.path === "default" ? t("portals.files.scopeOrganization") : t("portals.files.scopePortal");
  return (
    <div className="space-y-4">
      <p className="text-caption text-fg-muted">{scope}</p>
      <Card className="space-y-3">
        <FileDropZone
          label={t("portals.files.choose")}
          button={t("portals.files.choose")}
          hint={t("portals.files.hint.drop", { max: sizeText(limits.maxFileBytes) })}
          accept="image/png,image/jpeg,image/webp,image/svg+xml,application/pdf,text/csv,.csv,.xlsx"
          multiple
          primary={items.length === 0}
          onFiles={(picked) => {
            if (!uploading) void upload(picked);
          }}
        >
          {uploading ? (
            <div role="status" aria-live="polite" className="w-full space-y-1">
              <p className="text-caption text-fg">{t("portals.files.uploading", { done: uploading.done + 1, total: uploading.total, name: uploading.name })}</p>
              {/* fetch reports no upload progress, so the bar counts files: the one on its way fills half its share, pulsing. */}
              <div
                role="progressbar"
                aria-label={t("portals.files.progress")}
                aria-valuemin={0}
                aria-valuemax={uploading.total}
                aria-valuenow={uploading.done}
                className="h-2 w-full max-w-md overflow-hidden rounded-full bg-surface-muted"
              >
                <div className="h-full animate-pulse rounded-full bg-primary" style={{ width: `${Math.round(((uploading.done + 0.5) / uploading.total) * 100)}%` }} />
              </div>
            </div>
          ) : null}
        </FileDropZone>
        <p className={full ? "text-caption text-danger" : "text-caption text-fg-muted"}>
          {t("portals.files.limits", { files: limits.files, maxFiles: limits.maxFiles, bytes: sizeText(limits.bytes), maxBytes: sizeText(limits.maxBytes) })}
          {full ? ` ${t("portals.files.full")}` : ""}
        </p>
      </Card>
      {notice ? (
        <Alert tone={notice.tone} role={notice.tone === "danger" ? "alert" : "status"}>
          {notice.text}
        </Alert>
      ) : null}
      {deleting ? (
        <Alert tone="danger" role="alert" title={t("portals.files.deleteTitle", { name: deleting.name })}>
          <p>{t("portals.files.deleteBody")}</p>
          <div className="mt-3 flex flex-wrap gap-2">
            <Button size="sm" variant="danger" loading={remove.isPending} onClick={() => remove.mutate(deleting)}>
              {t("portals.files.deleteConfirm")}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setDeleting(null)}>
              {t("portals.sources.cancel")}
            </Button>
          </div>
        </Alert>
      ) : null}
      {renaming ? (
        <Card>
          <form
            className="flex flex-wrap items-end gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              rename.mutate(renaming);
            }}
          >
            <Field id="file-rename" label={t("portals.files.newName")} description={t("portals.files.hint.name")} required>
              <Input id="file-rename" value={renaming.name} maxLength={120} onChange={(event) => setRenaming({ ...renaming, name: event.target.value })} />
            </Field>
            <Button type="submit" variant="primary" loading={rename.isPending} disabled={renaming.name.trim() === ""} title={renaming.name.trim() === "" ? t("portals.files.hint.name") : undefined}>
              {t("portals.files.rename")}
            </Button>
            <Button variant="ghost" onClick={() => setRenaming(null)}>
              {t("portals.sources.cancel")}
            </Button>
          </form>
        </Card>
      ) : null}
      {items.some((f) => f.usedBy.length > 0) ? <p className="text-caption text-fg-muted">{t("portals.files.usedNote")}</p> : null}
      <ManifestGrid<FileRow>
        caption={t("portals.files.caption")}
        storageKey="portal-files"
        searchLabel={t("grid.searchIn", { list: t("portals.files.caption") })}
        rows={items}
        rowKey={(f) => f.id}
        query={{ isPending: false, isError: false, error: null, refetch: () => void files.refetch() }}
        name={{
          id: "name",
          label: t("portals.files.name"),
          text: (f) => f.name,
          cell: (f) => (
            <span className="break-all font-medium text-fg">{f.name}</span>
          ),
        }}
        columns={[
          { id: "type", label: t("portals.files.type"), filter: true, text: (f) => typeText(f.contentType), cell: (f) => <Badge tone="neutral">{typeText(f.contentType)}</Badge> },
          { id: "size", label: t("portals.files.size"), numeric: true, align: "right", text: (f) => sizeText(f.size) },
          {
            id: "usedBy",
            label: t("portals.files.usedBy"),
            text: (f) => (f.usedBy.length === 0 ? t("portals.files.unused") : f.usedBy.join(", ")),
            cell: (f) => (f.usedBy.length === 0 ? <span className="text-fg-muted">{t("portals.files.unused")}</span> : <span className="break-words">{f.usedBy.join(", ")}</span>),
          },
          { id: "uploadedBy", label: t("portals.files.uploadedBy"), text: (f) => f.uploadedBy },
          { id: "uploadedAt", label: t("portals.files.uploadedAt"), text: (f) => new Date(f.uploadedAt).toLocaleString(i18n.language) },
          { id: "path", label: t("portals.files.address"), hidden: true, text: (f) => f.path, cell: (f) => <code className="break-all text-caption">{f.path}</code> },
        ]}
        actions={(f) => (
          <div className="flex flex-wrap gap-1">
            <Button size="sm" variant="ghost" aria-label={t("portals.files.renameNamed", { name: f.name })} onClick={() => setRenaming({ id: f.id, name: f.name })}>
              {t("portals.files.rename")}
            </Button>
            <ReplaceButton file={f} disabled={uploading !== null || f.usedBy.length > 0} onFile={(file) => void upload([file], f)} />
            <Button
              size="sm"
              variant="ghost"
              className="text-danger"
              aria-label={t("portals.files.deleteNamed", { name: f.name })}
              disabled={f.usedBy.length > 0}
              title={f.usedBy.length > 0 ? t("portals.files.inUse", { where: f.usedBy.join(", ") }) : undefined}
              onClick={() => setDeleting(f)}
            >
              {t("portals.files.delete")}
            </Button>
          </div>
        )}
        empty={uploading ? <p className="text-caption text-fg-muted">{t("portals.files.firstUpload")}</p> : <EmptyState icon="inbox" title={t("portals.files.empty")} description={t("portals.files.emptyHint")} />}
      />
    </div>
  );
}

function ReplaceButton({ file, disabled, onFile }: { file: FileRow; disabled: boolean; onFile: (file: File) => void }): JSX.Element {
  const { t } = useTranslation();
  const why = file.usedBy.length > 0 ? t("portals.files.inUse", { where: file.usedBy.join(", ") }) : t("portals.files.busy");
  if (disabled) {
    return (
      <Button size="sm" variant="ghost" disabled title={why}>
        {t("portals.files.replace")}
      </Button>
    );
  }
  return (
    <FilePicker label={t("portals.files.replaceNamed", { name: file.name })} onFile={onFile}>
      <span className="inline-flex h-8 items-center rounded-md px-2 text-caption font-medium text-fg hover:bg-surface-subtle">{t("portals.files.replace")}</span>
    </FilePicker>
  );
}
