import { useEffect, useId, useState } from "react";
import type { JSX } from "react";
import { useMutation } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../api/client";
import { Alert, Button, Checkbox, Dialog, Field, Icon, Textarea } from "./ui";

/** The longest text the API keeps (API/01 §38). */
const MAX_TEXT = 2000;
/** The widest screenshot sent: enough to read a page, small enough for the 2 MB the API takes. */
const MAX_WIDTH = 1600;

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * Where to paint over, in the image's pixels: every field a person types into, scaled from the
 * page's layout to the captured frame. A field off screen is left out.
 */
export function maskRects(fields: Rect[], scale: number, frame: { width: number; height: number }): Rect[] {
  return fields
    .map((field) => ({
      x: Math.max(0, Math.floor(field.x * scale)),
      y: Math.max(0, Math.floor(field.y * scale)),
      width: Math.ceil(field.width * scale),
      height: Math.ceil(field.height * scale),
    }))
    .filter((rect) => rect.width > 0 && rect.height > 0 && rect.x < frame.width && rect.y < frame.height);
}

/** What a person may have typed something private into, on the page under the dialog. */
function fieldsOnPage(): Rect[] {
  return [...document.querySelectorAll<HTMLElement>("input, textarea, select, [contenteditable=''], [contenteditable='true'], [data-sensitive]")]
    .map((element) => element.getBoundingClientRect())
    .map((box) => ({ x: box.left, y: box.top, width: box.width, height: box.height }));
}

/**
 * One frame of this tab, as the browser lets the person share it, with every field painted over
 * (T-3272). The browser asks the person what to share; nothing is captured without that answer.
 */
async function screenshot(): Promise<string> {
  const media = navigator.mediaDevices as MediaDevices & {
    getDisplayMedia?: (options: Record<string, unknown>) => Promise<MediaStream>;
  };
  if (!media?.getDisplayMedia) throw new Error("unsupported");
  const stream = await media.getDisplayMedia({ video: { displaySurface: "browser" }, audio: false, preferCurrentTab: true });
  try {
    const video = document.createElement("video");
    video.muted = true;
    video.srcObject = stream;
    await video.play();
    const scale = Math.min(1, MAX_WIDTH / video.videoWidth);
    const canvas = document.createElement("canvas");
    canvas.width = Math.round(video.videoWidth * scale);
    canvas.height = Math.round(video.videoHeight * scale);
    const context = canvas.getContext("2d");
    if (!context) throw new Error("unsupported");
    context.drawImage(video, 0, 0, canvas.width, canvas.height);
    // The theme's own muted colour, so the painted fields read as covered rather than as content.
    context.fillStyle = getComputedStyle(document.documentElement).getPropertyValue("--color-fg-muted").trim() || "gray";
    for (const rect of maskRects(fieldsOnPage(), canvas.width / window.innerWidth, canvas)) {
      context.fillRect(rect.x, rect.y, rect.width, rect.height);
    }
    return canvas.toDataURL("image/png");
  } finally {
    stream.getTracks().forEach((track) => track.stop());
  }
}

/**
 * Feedback from any page (T-3272, API/01 §38): what got in the way, in the person's words, with
 * the page it happened on. Nobody is named; a screenshot only when ticked, its fields painted over.
 *
 * The Shell holds `open` so the Help menu can open the same dialog: on a phone the header has no
 * room for the button beside 44 px targets (T-3319), so `buttonClassName` hides it there and the
 * Help menu carries "Send feedback" instead.
 */
export function FeedbackButton({
  open: held,
  onOpenChange,
  buttonClassName,
}: {
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  buttonClassName?: string;
} = {}): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const [own, setOwn] = useState(false);
  const open = held ?? own;
  const setOpen = (next: boolean) => {
    setOwn(next);
    onOpenChange?.(next);
  };
  const [text, setText] = useState("");
  const [withScreenshot, setWithScreenshot] = useState(false);
  const [capturing, setCapturing] = useState(false);
  const [said, setSaid] = useState("");

  const send = useMutation({
    mutationFn: async () => {
      let image: string | undefined;
      if (withScreenshot) {
        // The dialog steps aside so the frame is the page, not the dialog over it.
        setCapturing(true);
        await new Promise((resolve) => requestAnimationFrame(resolve));
        try {
          image = await screenshot();
        } catch (err) {
          throw new Error(
            err instanceof Error && err.message === "unsupported" ? t("feedback.screenshotUnsupported") : t("feedback.screenshotRefused"),
          );
        } finally {
          setCapturing(false);
        }
      }
      return unwrap(
        await api.POST("/api/v1/feedback", {
          body: { text, page: window.location.pathname, ...(image ? { screenshot: image } : {}) },
        }),
      );
    },
    onSuccess: () => {
      setOpen(false);
      setText("");
      setWithScreenshot(false);
      setSaid(t("feedback.sent"));
    },
  });

  // Every opening starts clean, whichever control opened it: the last answer is cleared while
  // rendering the opening (React's way to follow a changed prop, no effect), and the mutation's
  // own state in an effect, since it is not this component's state.
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) setSaid("");
  }
  const reset = send.reset;
  useEffect(() => {
    if (open) reset();
  }, [open, reset]);

  const failure =
    send.error instanceof ApiError ? (send.error.problem?.detail ?? send.error.message) : send.error ? send.error.message : null;
  const tooLong = text.length > MAX_TEXT;

  return (
    <>
      <span role="status" aria-live="polite" className="sr-only">
        {said}
      </span>
      <span className={buttonClassName}>
        <Button
          variant="ghost"
          className="px-1.5"
          aria-label={t("feedback.button")}
          title={t("feedback.button")}
          onClick={() => setOpen(true)}
        >
          <Icon name="chat" className="size-5" />
        </Button>
      </span>
      <Dialog
        open={open && !capturing}
        onOpenChange={(next) => {
          if (!send.isPending) setOpen(next);
        }}
        title={t("feedback.title")}
        description={t("feedback.lead")}
        closeLabel={t("feedback.close")}
        footer={
          <>
            <Button variant="secondary" onClick={() => setOpen(false)} disabled={send.isPending} disabledReason={t("app.busyReason")}>
              {t("form.cancel")}
            </Button>
            <Button
              variant="primary"
              loading={send.isPending}
              disabled={text.trim() === "" || tooLong}
              disabledReason={text.trim() === "" ? t("feedback.needText") : tooLong ? t("feedback.tooLong", { max: MAX_TEXT }) : undefined}
              onClick={() => send.mutate()}
            >
              {t("feedback.send")}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-4">
          <Field
            id={`${id}-text`}
            label={t("feedback.text")}
            help={t("feedback.textHelp", { page: window.location.pathname })}
            errors={tooLong ? [t("feedback.tooLong", { max: MAX_TEXT })] : undefined}
          >
            <Textarea id={`${id}-text`} rows={5} value={text} onChange={(event) => setText(event.target.value)} />
          </Field>
          <div className="flex flex-col gap-1">
            <Checkbox
              label={t("feedback.screenshot")}
              aria-describedby={`${id}-shot-help`}
              checked={withScreenshot}
              onChange={(event) => setWithScreenshot(event.target.checked)}
            />
            <p id={`${id}-shot-help`} className="text-caption text-fg-muted">
              {t("feedback.screenshotHelp")}
            </p>
          </div>
          {failure ? (
            <Alert tone="danger" role="alert">
              {failure}
            </Alert>
          ) : null}
        </div>
      </Dialog>
    </>
  );
}
