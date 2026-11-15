# Environment layers

A layer holds value files that several environments share, so a fix lands once (T-4956). An
environment names its layers in its `global.yaml.gotmpl`:

```yaml
environmentLayers:
  - hosted
```

`helmfile-components.yaml.gotmpl` loads, in this order: `defaults/environment/*.yaml.gotmpl`,
then each layer's `*.yaml.gotmpl`, then the environment's own files, which win. A value that
differs between environments stays in the environment's file; a file that both environments
hold byte for byte moves here.

- `hosted`: the two hosted clusters, `dev` and `smartcity` (production).
