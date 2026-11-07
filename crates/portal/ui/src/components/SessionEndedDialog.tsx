import { useState } from "react";
import type { JSX } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api } from "../api/client";
import { clearSessionEnded, useSessionEnded } from "../api/sessionEnded";
import { Button, buttonClass, Dialog, InlineError, safeHref } from "./ui";

/**
 * "Your session ended" over the page that noticed it (UI-16, T-2747).
 *
 * The page stays as it is, with what the person typed: they sign in again in a new tab, come
 * back and continue, and every read is asked again. Signing in here instead leaves the page,
 * and the dialog says what that costs before they choose it.
 */
export function SessionEndedDialog(): JSX.Element | null {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const loginUrl = useSessionEnded();
  const [opened, setOpened] = useState(false);
  const [notYet, setNotYet] = useState(false);
  const [checking, setChecking] = useState(false);

  if (loginUrl === null) return null;

  const close = () => {
    setOpened(false);
    setNotYet(false);
    clearSessionEnded();
  };

  const carryOn = async () => {
    setChecking(true);
    try {
      // Through the typed client (UI-07); a 401 on /auth/me does not announce the session
      // ended again, so asking cannot reopen this dialog on itself.
      const { response } = await api.GET("/api/v1/auth/me");
      if (response.ok) {
        close();
        await queryClient.invalidateQueries();
      } else {
        setNotYet(true);
      }
    } catch {
      setNotYet(true);
    } finally {
      setChecking(false);
    }
  };

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) close();
      }}
      title={t("app.session.title")}
      description={t("app.session.body")}
      closeLabel={t("app.session.close")}
      size="sm"
      footer={
        <>
          <Button variant="ghost" onClick={close}>
            {t("app.session.close")}
          </Button>
          {opened ? (
            <Button variant="primary" loading={checking} onClick={() => void carryOn()}>
              {t("app.session.continue")}
            </Button>
          ) : (
            <a
              href={safeHref(loginUrl)}
              target="_blank"
              rel="noopener noreferrer"
              className={buttonClass("primary")}
              onClick={() => setOpened(true)}
            >
              {t("app.session.signIn")}
            </a>
          )}
        </>
      }
    >
      <p className="text-body">{opened ? t("app.session.afterSignIn") : t("app.session.hereCost")}</p>
      <Button variant="secondary" size="sm" className="mt-3" onClick={() => window.location.assign(loginUrl)}>
        {t("app.session.here")}
      </Button>
      {notYet ? (
        <InlineError className="mt-3">
          {t("app.session.notYet")}
        </InlineError>
      ) : null}
    </Dialog>
  );
}
