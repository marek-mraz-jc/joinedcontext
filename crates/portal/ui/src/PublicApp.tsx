import { Suspense, lazy, useMemo } from "react";
import { RouterProvider } from "@tanstack/react-router";
import { useQueryClient } from "@tanstack/react-query";
import { AuthProvider, useAuth } from "./auth/AuthProvider";
import { BrandingProvider } from "./branding";
import { loadFullLocales } from "./i18n";
import { createPublicRouter } from "./publicRouter";

/**
 * A signed-in person on a public page gets the Portal's own frame for it, the shell and their
 * project, so the full router loads, with every page's words, once the session says so.
 */
const FullPortal = lazy(() =>
  Promise.all([import("./App"), import("./router"), loadFullLocales()]).then(([app, router]) => ({
    default: function FullPortal() {
      const portal = useMemo(() => router.createPortalRouter(), []);
      return <app.RoutedApp router={portal} />;
    },
  })),
);

function PublicRouted(): React.JSX.Element {
  const auth = useAuth();
  const queryClient = useQueryClient();
  const router = useMemo(() => createPublicRouter(queryClient), [queryClient]);
  // A visitor's page draws while the session is asked for: no public route has a guard to hold, and
  // the answer is a round trip a phone would otherwise wait before the first word.
  if (auth.status === "authenticated") {
    return (
      <Suspense fallback={<main aria-busy="true" className="min-h-screen bg-bg" />}>
        <FullPortal />
      </Suspense>
    );
  }
  return <RouterProvider router={router} />;
}

/** What main.tsx draws for an address anybody may open (T-5122, publicRouter.tsx). */
export function PublicApp(): React.JSX.Element {
  return (
    <BrandingProvider>
      <AuthProvider>
        <PublicRouted />
      </AuthProvider>
    </BrandingProvider>
  );
}
