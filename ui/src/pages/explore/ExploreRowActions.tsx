import { useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ApiError } from "../../api/client";
import { deleteEntity, fetchEntity, MAX_REASON, reasonProblem } from "../../components/entities/filters";
import { Alert, Button, CodeBlock, Field, Input } from "../../components/ui";

/**
 * What the explorer adds under a row's fields in the grid's own detail (T-5619): the removal of
 * the entity (UI-60) with the reason it is removed for (DM-113), and the entity as the endpoint
 * holds it, folded away as technical details. Both go through the endpoint that showed the row,
 * with the person's session and its Policy.
 */
export function ExploreRowActions({
  slug,
  id,
  deniedReason,
  onDeleted,
}: {
  slug: string;
  id: string;
  /** Why the endpoint's grant refuses the removal, or nothing when it allows it (UI-44). */
  deniedReason?: string;
  onDeleted: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [removing, setRemoving] = useState(false);
  const [reason, setReason] = useState("");
  const [tried, setTried] = useState(false);
  const opener = useRef<HTMLButtonElement>(null);
  const remove = useMutation({ mutationFn: () => deleteEntity(slug, id, reason), onSuccess: onDeleted });
  const reasonError = tried ? reasonProblem(reason) : null;
  const failed = remove.error instanceof ApiError ? remove.error.message : remove.error ? t("app.error.generic") : null;

  return (
    <div className="mt-4 flex flex-col gap-3">
      <TechnicalDetails slug={slug} id={id} />
      <div>
        <Button
          size="sm"
          variant="danger"
          ref={opener}
          data-testid="explore-delete"
          aria-expanded={removing}
          aria-controls="explore-remove"
          disabled={deniedReason !== undefined}
          disabledReason={deniedReason}
          onClick={() => {
            remove.reset();
            setReason("");
            setTried(false);
            setRemoving(true);
          }}
        >
          {t("explore.delete")}
        </Button>
      </div>
      {removing ? (
        // In the dialog itself: a second dialog over the browser's modal one would sit behind it, inert.
        <section id="explore-remove" aria-labelledby="explore-remove-title" className="flex flex-col gap-3 rounded-md border border-danger p-3">
          <h4 id="explore-remove-title" className="text-body font-semibold text-fg">
            {t("explore.removeTitle")}
          </h4>
          <p className="text-caption text-fg-muted">{t("explore.removeNote")}</p>
          <Field
            id="explore-delete-reason"
            label={t("explore.removeReason")}
            required
            help={t("explore.removeReasonHint")}
            errors={reasonError ? [t(`explore.removeReasonError.${reasonError}`, { max: MAX_REASON })] : undefined}
          >
            <Input
              id="explore-delete-reason"
              value={reason}
              maxLength={MAX_REASON}
              autoComplete="off"
              onChange={(event) => setReason(event.target.value)}
            />
          </Field>
          {failed ? (
            <Alert role="alert" tone="danger">
              {failed}
            </Alert>
          ) : null}
          <div className="flex flex-wrap gap-2">
            <Button
              variant="danger"
              size="sm"
              disabled={remove.isPending}
              disabledReason={t("app.busyReason")}
              data-testid="explore-delete-confirm"
              onClick={() => {
                setTried(true);
                if (reasonProblem(reason) === null) remove.mutate();
              }}
            >
              {t("explore.removeConfirm")}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => {
                setRemoving(false);
                // The section goes away with the focus in it: back to what opened it.
                opener.current?.focus();
              }}
            >
              {t("form.cancel")}
            </Button>
          </div>
        </section>
      ) : null}
    </div>
  );
}

/** The entity's JSON-LD, read only once someone opens the disclosure, with a copy of it. */
function TechnicalDetails({ slug, id }: { slug: string; id: string }): JSX.Element {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const entity = useQuery({
    queryKey: ["explore-entity", slug, id],
    queryFn: () => fetchEntity(slug, id),
    enabled: open,
  });
  const text = entity.data === undefined ? "" : JSON.stringify(entity.data, null, 2);
  return (
    <details onToggle={(event) => setOpen(event.currentTarget.open)} data-testid="explore-technical">
      <summary className="cursor-pointer text-caption text-fg-muted">{t("explore.technical")}</summary>
      <div className="mt-2 flex flex-col gap-2" aria-live="polite" aria-busy={(open && entity.isPending) || undefined}>
        {entity.error ? (
          <Alert role="alert" tone="danger">
            {entity.error instanceof ApiError ? entity.error.message : t("explore.loadFailed")}
          </Alert>
        ) : text ? (
          <>
            <div>
              <Button
                size="sm"
                onClick={() => {
                  void navigator.clipboard?.writeText(text).then(() => setCopied(true), () => setCopied(false));
                }}
              >
                {copied ? t("endpoints.copied") : t("explore.copyJson")}
              </Button>
            </div>
            <CodeBlock label={t("codeBlock.entity")} className="max-h-72 rounded bg-surface-subtle p-2 font-mono text-caption">
              {text}
            </CodeBlock>
          </>
        ) : open ? (
          <p className="text-caption text-fg-muted">{t("app.loading")}</p>
        ) : null}
      </div>
    </details>
  );
}
