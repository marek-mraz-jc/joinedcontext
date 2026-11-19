# sharing smoke model

The four artifacts a published DataModel commits (DM-02) for the suite's shared model `ShareCheck`
(`linkml()` in `../suite.py`). The suite copies them beside each space's DataModel.

Regenerate after changing `linkml()`: write one space with `space_files()` into an empty directory
with the configuration's `platform-settings.yaml`, then run
`JC_MODEL_TOOLS_URL=<a local jc's model-tools address> jcctl model generate --repo-dir <dir>` and copy
the four `sh-model.v1.*` files here (A's full model) and, from a space of project `sh-b`, the
four `sh-model-b.v1.*` (B's, only the attributes A shares). The copies in every space are byte-equal: the artifacts do not
depend on the DataModel's name or space. Generated 2026-11-19 by jc-model-tools-0.1.0 (T-7402).
