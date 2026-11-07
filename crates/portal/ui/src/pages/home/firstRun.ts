/**
 * Whether the first-run checklist is shown (T-3233): per person, in their stored preferences,
 * so it follows them across projects and browsers. Without a preferences database the checklist
 * is shown and putting it away lasts the session.
 */
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, queryKeys, unwrap } from "../../api/client";

export function useFirstRun(): { shown: boolean; dismiss: () => void; restore: () => void } {
  const queryClient = useQueryClient();
  const [local, setLocal] = useState<boolean | null>(null);
  const preferences = useQuery({
    queryKey: queryKeys.preferences(),
    retry: false,
    queryFn: async () => unwrap(await api.GET("/api/v1/preferences", {})),
  });
  const save = useMutation({
    mutationFn: async (dismissed: boolean) =>
      unwrap(await api.PUT("/api/v1/preferences", { body: { ...(preferences.data ?? {}), firstRunDismissed: dismissed } })),
    onSuccess: (stored) => queryClient.setQueryData(queryKeys.preferences(), stored),
  });
  const dismissed = local ?? preferences.data?.firstRunDismissed === true;
  return {
    // Not before the preferences answered (or failed): a checklist that flashed up and vanished
    // again for a person who put it away is noise.
    shown: !dismissed && (local !== null || !preferences.isPending),
    dismiss: () => {
      setLocal(true);
      save.mutate(true);
    },
    restore: () => {
      setLocal(false);
      save.mutate(false);
    },
  };
}
