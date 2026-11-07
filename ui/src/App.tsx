import { useMemo } from "react";
import { RouterProvider } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { AuthProvider, useAuth } from "./auth/AuthProvider";
import { BrandingProvider } from "./branding";
import { SessionEndedDialog } from "./components/SessionEndedDialog";
import { StepUpDialog } from "./components/StepUpDialog";
import { createPortalRouter } from "./router";

export function RoutedApp({ router }: { router: ReturnType<typeof createPortalRouter> }) {
  const auth = useAuth();
  const { t } = useTranslation();

  // The route guards read `context.auth` in `beforeLoad`, which runs once per navigation.
  // Mounting the router before the session settles would let an anonymous visitor through the
  // guard on the very first match, so hold the tree until the answer is in.
  if (auth.status === "loading") {
    return (
      <p role="status" className="p-6 font-sans text-surface-fg">
        {t("app.loading")}
      </p>
    );
  }

  return (
    <>
      <RouterProvider router={router} context={{ auth }} />
      {/* Only a person who was signed in can lose a session; an anonymous visitor's 401s are
          the router's way to the login page. */}
      {auth.status === "authenticated" ? <SessionEndedDialog /> : null}
      {auth.status === "authenticated" ? <StepUpDialog /> : null}
    </>
  );
}

export function App(): React.JSX.Element {
  const router = useMemo(() => createPortalRouter(), []);
  return (
    <BrandingProvider>
      <AuthProvider>
        <RoutedApp router={router} />
      </AuthProvider>
    </BrandingProvider>
  );
}

export default App;
