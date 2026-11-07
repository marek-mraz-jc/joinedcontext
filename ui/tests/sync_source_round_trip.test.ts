// covers: src/pages/sync/SyncSourcesPage.tsx
/** F-UI3-018: the YAML view and the form are one resource; a round trip keeps every field. */
import { describe, expect, it } from "vitest";
import { syncSourceFromManifest, syncSourceToManifest } from "../src/pages/sync/SyncSourcesPage";

describe("a sync source read back from its manifest", () => {
  it("keeps prune, autoMerge and the title", () => {
    const form = {
      name: "upstream",
      title: "City upstream",
      git: { url: "https://git.example/city.git", ref: "main" },
      interval: "1h",
      mode: "mirror",
      conflictPolicy: "fail",
      prune: true,
      autoMerge: true,
    };
    expect(syncSourceFromManifest(syncSourceToManifest("helsinki", form))).toEqual(form);
  });
});
