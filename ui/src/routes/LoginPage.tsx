import { useEffect } from "react";
import type { JSX } from "react";
import { useRouter } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { useAuth } from "../auth/AuthProvider";
import { useBranding } from "../branding";
import { LanguageSwitcher } from "../components/LanguageSwitcher";
import { BrandMark } from "../components/layout/BrandMark";
import { Button, Card, Icon } from "../components/ui";
import { sameOriginPath } from "../components/ui/safeHref";

/** Where to land after the login: the location the session ran out on, never another origin. */
export function redirectTarget(search: string): string {
  const candidate = new URLSearchParams(search).get("redirect_to");
  return (candidate?.startsWith("/") && sameOriginPath(candidate)) || "/";
}

export function LoginPage(): JSX.Element {
  const { t } = useTranslation();
  const { signIn, status } = useAuth();
  const branding = useBranding();
  const router = useRouter();

  // Behind the edge, Keycloak returns a person to the address they opened, and that can be this
  // page: somebody signed in goes on to where they were going instead of meeting a Sign in
  // button that does nothing for them (T-2729). Only to this origin, as for the sign-in itself.
  useEffect(() => {
    if (status === "authenticated") router.history.replace(redirectTarget(window.location.search));
  }, [status, router]);

  if (status === "authenticated") return <main aria-busy="true" className="min-h-screen bg-bg" />;

  return (
    <main className="relative flex min-h-screen items-center justify-center overflow-hidden bg-bg px-4 py-10 font-sans text-fg">
      {/* Two soft washes of the brand colour; decorative, and the same tokens as everything. */}
      <div
        aria-hidden="true"
        className="pointer-events-none absolute -left-32 -top-32 size-[28rem] rounded-full bg-primary-100 blur-3xl"
      />
      <div
        aria-hidden="true"
        className="pointer-events-none absolute -bottom-40 -right-24 size-[24rem] rounded-full bg-primary-50 blur-3xl"
      />

      <div className="relative w-full max-w-sm">
        <div className="mb-4 flex justify-end">
          <LanguageSwitcher />
        </div>
        <Card flush className="rounded-xl p-8 shadow-2">
          <h1 className="text-title font-semibold">
            <BrandMark />
          </h1>
          <p className="mt-3 text-body text-fg-muted">{t("auth.lead")}</p>
          <Button
            variant="primary"
            size="lg"
            className="mt-8 w-full"
            icon={<Icon name="user" className="size-4" />}
            onClick={() => {
              signIn(redirectTarget(window.location.search));
            }}
          >
            {t("auth.signIn")}
          </Button>
        </Card>
        {branding.organisation ? (
          <p className="mt-4 text-center text-caption text-fg-subtle">{branding.organisation}</p>
        ) : null}
      </div>
    </main>
  );
}
