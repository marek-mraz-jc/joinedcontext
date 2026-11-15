# WASM host

The shared host of server WASM Apps (ADR-N-044, AP-143). `component.yaml` lists its parts:

- `shard-0`, `shard-1`: one host per shard, named as the Portal places Apps
  (`JC_PORTAL_WASM_SHARDS`) and as `apps-db` makes their logins.
- `portal-access`: the Portal's Role in this namespace: it writes each shard's placement
  ConfigMap and store-key Secret (AP-157, AP-158), nothing else.
- `tokens`: per run, a token of the App's job principal by TokenRequest (AP-159, T-3372); off
  until the federated mechanism is on.
- `identities`: the namespace of the Apps' job ServiceAccounts and the token service's grant.

The hosted clusters share their values in `ci/example-deployments/environments/_layers/hosted/wasm-host.yaml.gotmpl`.

Deploy the component alone with:
$ # navigate to `deployment/` and run:
$ helmfile apply -i --selector component=wasm-host
