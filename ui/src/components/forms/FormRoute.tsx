import { createContext, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { HTMLAttributes, JSX, ReactNode } from "react";
import { createPortal } from "react-dom";
import { useIsFetching, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { Alert, Button, Card, Dialog, PageHeader, PageLoading } from "../ui";
import type { DialogSize } from "../ui";
import { failureKind } from "../ui/PageState";
import { proposesAnything, usePermissions } from "../../api/permissions";

/**
 * A bigger form is a page with an address of its own, not a popup (T-2474, UI-27).
 *
 * `/projects/{project}/{plural}/new` opens a kind's create form and
 * `/projects/{project}/{plural}/{name}/edit` its edit form, in the content area under the sidebar
 * and the header. Reloading lands on the same form, the link can be shared, and the browser's
 * back button returns to the list. The addresses carry identifiers only: what the form holds is
 * the person's draft, never the URL.
 *
 * The list pages keep their forms: a page renders its list and its form component as before, and
 * the route around it decides where the form goes. Inside a `FormRouteHost` the form is drawn
 * into the host's slot in place of the list; anywhere else (a component rendered on its own) it
 * stays the dialog it was.
 */
export type FormTarget = { mode: "new" } | { mode: "edit"; name: string };

interface FormRouteValue {
  /** The form the address names, or `null` on the list itself. */
  form: FormTarget | null;
  openNew: () => void;
  openEdit: (name: string) => void;
  /** The address `openEdit` goes to, for a real link to it (T-2875). */
  editHref: (name: string) => string;
  /** Back to the list, the way the list was left. */
  close: () => void;
  /**
   * The form saved and is about to close: what the save answered (the change it opened) is shown
   * above the list it goes back to, the way the list pages show their own (UI-23).
   */
  leave: (notice: ReactNode) => void;
  /** Where an open form is drawn. */
  slot: HTMLElement | null;
  /** An open form says so, so the host hides the list while it is there. */
  register: () => () => void;
}

const FormRouteContext = createContext<FormRouteValue | null>(null);

/** The routed form of the page this renders in, or `null` outside a `FormRouteHost`. */
export function useFormRoute(): FormRouteValue | null {
  return useContext(FormRouteContext);
}

/**
 * Whether the kind's create form is open, and the way to open or close it: the address when the
 * page is routed, the component's own state when it is not.
 */
export function useCreateForm(): [boolean, (open: boolean) => void] {
  const route = useFormRoute();
  const [own, setOwn] = useState(false);
  if (!route) {
    return [own, setOwn];
  }
  return [
    route.form?.mode === "new",
    (open) => {
      if (open) {
        route.openNew();
      } else if (route.form !== null) {
        route.close();
      }
    },
  ];
}

/**
 * The create form as `useCreateForm` gives it, opened as well on the draft the assistant handed
 * the page (`?draft=`, AG-61, AG-73): the form loads that draft by the name returned here. The
 * address is read once, as the page mounts; closing the form lets the draft go.
 */
export function useCreateFormFromDraft(): [boolean, (open: boolean) => void, string | undefined] {
  const [routedOpen, setRoutedOpen] = useCreateForm();
  const [handed] = useState(() =>
    typeof window === "undefined"
      ? undefined
      : (new URLSearchParams(window.location.search).get("draft") || undefined),
  );
  const [fromDraft, setFromDraft] = useState(handed !== undefined);
  return [
    routedOpen || fromDraft,
    (open) => {
      if (!open) {
        setFromDraft(false);
      }
      setRoutedOpen(open);
    },
    fromDraft ? handed : undefined,
  ];
}

/**
 * The edit form of one named resource: open when the address names it. `null` outside a routed
 * page, where the caller keeps its own state.
 */
export function useEditForm(name: string): [boolean, (open: boolean) => void] | null {
  const route = useFormRoute();
  if (!route) {
    return null;
  }
  return [
    route.form?.mode === "edit" && route.form.name === name,
    (open) => {
      if (open) {
        route.openEdit(name);
      } else if (route.form !== null) {
        route.close();
      }
    },
  ];
}

/**
 * For a page that opens its own editor (a data source, a pipeline): the editor the address names,
 * opened once the page's list has answered, since an edit needs the stored resource. A name the
 * list does not hold opens nothing, and the host says so.
 */
export function useOpenFromAddress<T>(
  items: T[] | undefined,
  nameOf: (item: T) => string,
  open: { create: () => void; edit: (item: T) => void },
): void {
  const route = useFormRoute();
  const form = route?.form ?? null;
  // Which address was opened: the list stays mounted while its forms come and go, so the next
  // address opens its own editor.
  const done = useRef<string | null>(null);
  const latest = useRef({ nameOf, open });
  // The page's own callbacks change on every render; the effect below reads the newest ones.
  useEffect(() => {
    latest.current = { nameOf, open };
  });
  const key = form === null ? null : form.mode === "new" ? "new" : `edit:${form.name}`;
  useEffect(() => {
    if (key === null) {
      done.current = null;
      return;
    }
    if (done.current === key || form === null || items === undefined) {
      return;
    }
    if (form.mode === "new") {
      done.current = key;
      latest.current.open.create();
      return;
    }
    const found = items.find((item) => latest.current.nameOf(item) === form.name);
    if (found !== undefined) {
      done.current = key;
      latest.current.open.edit(found);
    }
  }, [key, form, items]);
}

/** Whether a form is drawn as a page, under the page's own `h1`, rather than in a dialog. */
const FormPageContext = createContext(false);

/**
 * A section heading inside a form: one level under the form's title wherever the form is drawn,
 * an `h2` under a page's `h1` and an `h3` under a dialog's `h2`, so the outline has no gap in
 * either (WCAG 1.3.1). `sub` is a heading one level further in.
 */
export function FormHeading({
  sub = false,
  ...props
}: { sub?: boolean } & HTMLAttributes<HTMLHeadingElement>): JSX.Element {
  const page = useContext(FormPageContext);
  const level = (page ? 2 : 3) + (sub ? 1 : 0);
  const Heading = level === 2 ? "h2" : level === 3 ? "h3" : "h4";
  return <Heading {...props} />;
}

export interface FormFrameProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: ReactNode;
  /** The dialog's width, when the form is drawn as one. */
  size?: DialogSize;
  /** The dialog's close control, when the form is drawn as one. */
  closeLabel: string;
  /** Beside the title: the kind's User Guide link (T-6105). */
  help?: ReactNode;
  footer?: ReactNode;
  children: ReactNode;
}

/**
 * The frame of a bigger form: a page in the routed slot, or the dialog it has always been where
 * no route hosts it. Closing the page goes back to the list's own address (the host does that).
 */
export function FormFrame({
  open,
  onOpenChange,
  title,
  description,
  size,
  closeLabel,
  help,
  footer,
  children,
}: FormFrameProps): JSX.Element | null {
  const route = useFormRoute();

  // The address left the form (the browser's back button, a link in the sidebar's section): the
  // form closes with it instead of staying over the list.
  const addressed = route ? route.form !== null : false;
  const wasAddressed = useRef(addressed);
  useEffect(() => {
    if (wasAddressed.current && !addressed && open) {
      onOpenChange(false);
    }
    wasAddressed.current = addressed;
  }, [addressed, open, onOpenChange]);

  // Registered before paint: a passive effect let the page show this form and the list behind it,
  // heading and all, for a frame, and a screen reader read both (T-3423).
  const register = route?.register;
  useLayoutEffect(() => {
    if (register && open) {
      return register();
    }
    return undefined;
  }, [register, open]);

  // The page takes the focus the way a dialog does, so a keyboard or a screen reader lands on the
  // form it opened and not on the hidden button that opened it.
  const page = useRef<HTMLElement>(null);
  const shown = route !== null && open && route.slot !== null;
  useEffect(() => {
    if (shown) {
      page.current?.focus();
    }
  }, [shown]);

  if (!route) {
    return (
      <Dialog
        open={open}
        onOpenChange={onOpenChange}
        title={title}
        description={description}
        size={size}
        closeLabel={closeLabel}
        titleAction={help}
        footer={footer}
      >
        {children}
      </Dialog>
    );
  }
  if (!open || !route.slot) {
    return null;
  }
  return createPortal(
    <section
      ref={page}
      tabIndex={-1}
      data-testid="form-page"
      aria-label={title}
      className="flex flex-col gap-6 focus:outline-none"
      // Escape leaves the form as it left the dialog (UI-47), with whatever question the form
      // asks first. A menu or a list box drawn in a portal handles its own Escape: it is not
      // inside this section, so it closes itself and not the form.
      onKeyDown={(event) => {
        if (
          event.key === "Escape" &&
          !event.defaultPrevented &&
          event.currentTarget.contains(event.target as Node)
        ) {
          event.preventDefault();
          onOpenChange(false);
        }
      }}
    >
      {/* The breadcrumb and Cancel lead back; the lead waits behind the title's info mark (owner 2026-10-31). */}
      <PageHeader compact title={title} description={description} titleAction={help} />
      <Card>
        <FormPageContext.Provider value>{children}</FormPageContext.Provider>
      </Card>
      {footer ? <div className="flex flex-wrap items-center justify-end gap-2">{footer}</div> : null}
    </section>,
    route.slot,
  );
}

/**
 * Whether the reads of the page at this form address have answered. Decided in an effect, which runs
 * after the children's: an editor registers in its own effect once the reads have answered, and a
 * verdict taken in that render said "cannot be opened" for a moment, an alert a screen reader
 * announced on the way to every form (T-3423). The effect records it once the commit is done, when
 * every registration of that commit has been counted.
 */
function useFormReads(formKey: string | null): { settled: boolean; readFailed: boolean } {
  const fetching = useIsFetching();
  // A read with nothing to show yet. A refresh of what the page holds (a poll, an invalidation)
  // drops no verdict: the page went behind the skeleton for it, its h1 with it (T-6525, UI-16).
  const reading = useIsFetching({ predicate: (query) => query.state.data === undefined });
  const queryClient = useQueryClient();
  const [settledFor, setSettledFor] = useState<string | null>(null);
  // A read the page needed failed (a server error, a lost connection): the page says so itself
  // with its Retry, and "no create form here" would be a second, wrong story (T-4165).
  const [readFailed, setReadFailed] = useState(false);
  useEffect(() => {
    // After this commit, the children's registrations included; a later change drops the verdict.
    let live = true;
    queueMicrotask(() => {
      if (!live) return;
      if (reading > 0) {
        setSettledFor(null);
        setReadFailed(false);
      } else if (fetching === 0) {
        setSettledFor(formKey);
        setReadFailed(
          queryClient
            .getQueryCache()
            .findAll({ type: "active" })
            .some((query) => {
              if (query.state.status !== "error") return false;
              const kind = failureKind(query.state.error);
              return kind === "server" || kind === "network";
            }),
        );
      }
    });
    return () => {
      live = false;
    };
  }, [fetching, reading, formKey, queryClient]);
  return { settled: reading === 0 && settledFor === formKey, readFailed };
}

/**
 * The list page and the slot its forms are drawn in. With a form in the address the list is
 * hidden (and out of the accessibility tree) while the form stands in its place; with a form in
 * the address that nothing opened (a name that is not there, a kind with no such form, an edit
 * the person may not make) the page says so once the page's reads have answered.
 */
export function FormRouteHost({
  project,
  plural,
  form,
  base,
  children,
}: {
  project: string;
  plural: string;
  form: FormTarget | null;
  /**
   * The list's own address when it is not `/projects/{project}/{plural}`: a tab of Project
   * settings hosts its forms at `{base}/new` and `{base}/{name}/edit` (T-2606).
   */
  base?: string;
  children: ReactNode;
}): JSX.Element {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const mayPropose = proposesAnything(usePermissions(project).data);
  const [slot, setSlot] = useState<HTMLDivElement | null>(null);
  const [open, setOpen] = useState(0);
  const [notice, setNotice] = useState<ReactNode>(null);
  // What had the focus when a form opened over the list, given it back once the list is shown.
  // Taken as the form is asked for, while the control that asked still holds the focus: once
  // the list is hidden a browser moves the focus off it.
  const returnTo = useRef<HTMLElement | null>(null);
  const remember = () => {
    returnTo.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  };

  const value = useMemo<FormRouteValue>(
    () => ({
      form,
      openNew: () => {
        remember();
        setNotice(null);
        if (base) {
          void navigate({ href: `${base}/new` });
          return;
        }
        void navigate({ to: "/projects/$project/$plural/new", params: { project, plural } });
      },
      openEdit: (name) => {
        remember();
        setNotice(null);
        if (base) {
          void navigate({ href: `${base}/${encodeURIComponent(name)}/edit` });
          return;
        }
        void navigate({
          to: "/projects/$project/$plural/$name/edit",
          params: { project, plural, name },
        });
      },
      editHref: (name) =>
        `${base ?? `/projects/${encodeURIComponent(project)}/${encodeURIComponent(plural)}`}/${encodeURIComponent(name)}/edit`,
      close: () => {
        if (base) {
          void navigate({ href: base });
          return;
        }
        void navigate({ to: "/projects/$project/$plural", params: { project, plural } });
      },
      leave: setNotice,
      slot,
      register: () => {
        setOpen((count) => count + 1);
        return () => setOpen((count) => count - 1);
      },
    }),
    [form, navigate, project, plural, base, slot],
  );

  const showing = open > 0;
  // The address names a form, the page's reads have answered, and nothing opened it.
  const formKey = form === null ? null : form.mode === "new" ? "new" : `edit:${form.name}`;
  const { settled, readFailed } = useFormReads(formKey);
  const unopened = form !== null && !showing && !readFailed && settled;
  // The last form closed, by its back control, its Cancel, a saved proposal or a page that
  // dropped its editor: the address leaves the form too, so a reload or the back button does
  // not land on a form that is no longer there. (The browser's back button leaves the address
  // first, and there is nothing left to leave.)
  const wasShowing = useRef(false);
  const addressed = form !== null;
  const close = value.close;
  useEffect(() => {
    const closed = wasShowing.current && !showing;
    wasShowing.current = showing;
    if (!closed) {
      return;
    }
    if (addressed) {
      close();
    }
    const back = returnTo.current;
    returnTo.current = null;
    if (back !== null && back.isConnected && back !== document.body) {
      back.focus();
    }
  }, [showing, addressed, close]);
  return (
    <FormRouteContext.Provider value={value}>
      {/* One block of the page's column, whatever it shows: the column spaces its children, so an
          empty slot beside the list would push the page down by a gap (T-2489). */}
      <div>
        {notice !== null && form === null && !showing ? <div className="mb-4">{notice}</div> : null}
        {/* An address whose form nothing opened says so above the list, which stays: the page
            keeps its heading and what does exist is one glance away (UI-16, T-2730). */}
        {unopened && form !== null ? (
          <div className="mb-4 flex flex-col gap-3">
            {/* A role that may propose nothing here reads why, not a red warning (T-4263). */}
            {form.mode === "new" && !mayPropose ? (
              <Alert tone="info" role="status">
                {t("form.mayNotCreate")}
              </Alert>
            ) : (
              <Alert tone="warning" role="alert">
                {/* A create address names no resource, and the list's plural is an API word, not
                    one a person reads (T-2750). */}
                {form.mode === "edit" ? t("form.notOpen", { name: form.name }) : t("form.notOpenNew")}
              </Alert>
            )}
            <div>
              <Button onClick={value.close}>{t("form.backToList")}</Button>
            </div>
          </div>
        ) : null}
        <div hidden={showing || (form !== null && !unopened && !readFailed)}>{children}</div>
        <div ref={setSlot} />
        {form !== null && !showing && !unopened && !readFailed ? (
          <PageLoading label={t("form.opening")} lines={2} />
        ) : null}
      </div>
    </FormRouteContext.Provider>
  );
}
