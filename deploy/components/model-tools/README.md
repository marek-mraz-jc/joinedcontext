# Model Tools

The stateless generator image behind the LinkML editor: LinkML compilation for the live
preview, Smart Data Models imports, mapping compilation and a draft model from a sample file
(`POST /infer-schema`, DM-54, DM-55). It holds no credential, reads no platform state and is
reachable from the Portal alone; the Portal finds it through `JC_PORTAL_MODEL_TOOLS_URL`,
set by the portal component when this component is in the list.

Deploy the component with:
```bash
helmfile apply -i --selector component=model-tools
```

## Image

`ghcr.io/marek-mraz-jc/joinedcontext-platform/model-tools`, pinned by digest, built from
`tools/model-tools` of the platform repository; entrypoint `python -m service`, port 8080,
health at `/healthz`.
