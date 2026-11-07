import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../api/client";
import { clearStepUp, useStepUp } from "../api/stepUp";
import { Button, buttonClass, Dialog, InlineError, safeHref } from "./ui";

/**
 * "This decision needs your one-time code" over the page whose decision was refused (T-5567).
 *
 * The person signs in again with the code in a new tab, comes back and presses the decision
 * again; the page keeps what they typed. Continue asks `/auth/me` whether the code arrived.
 */
export function StepUpDialog(): JSX.Element | null {
  const { t } = useTranslation();
  const loginUrl = useStepUp();
  const [opened, setOpened] = useState(false);
  const [notYet, setNotYet] = useState(false);
  const [checking, setChecking] = useState(false);

  if (loginUrl === null) return null;

  const close = () => {
    setOpened(false);
    setNotYet(false);
    clearStepUp();
  };

  const carryOn = async () => {
    setChecking(true);
    try {
      const { data } = await api.GET("/api/v1/auth/me");
      if (data?.second_factor) close();
      else setNotYet(true);
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
      title={t("app.stepUp.title")}
      description={t("app.stepUp.body")}
      closeLabel={t("app.stepUp.close")}
      size="sm"
      footer={
        <>
          <Button variant="ghost" onClick={close}>
            {t("app.stepUp.close")}
          </Button>
          {opened ? (
            <Button variant="primary" loading={checking} onClick={() => void carryOn()}>
              {t("app.stepUp.continue")}
            </Button>
          ) : (
            <a
              href={safeHref(loginUrl)}
              target="_blank"
              rel="noopener noreferrer"
              className={buttonClass("primary")}
              onClick={() => setOpened(true)}
            >
              {t("app.stepUp.signIn")}
            </a>
          )}
        </>
      }
    >
      <p className="text-body">{opened ? t("app.stepUp.afterSignIn") : t("app.stepUp.how")}</p>
      {notYet ? <InlineError className="mt-3">{t("app.stepUp.notYet")}</InlineError> : null}
    </Dialog>
  );
}
