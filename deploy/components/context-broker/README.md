# Context Broker

Antares ETSI GS CIM 009 NGSI-LD context broker component.

Deploy the component with:
```bash
helmfile apply -i --selector component=context-broker
```

## Further brokers

`main` is the release `context-broker-broker` (Service `context-broker`) on the platform's
`postgres-cluster`. Each entry of `global.contextBrokers` adds one more broker (ADR-N-046 §2.5,
T-3989):

```yaml
global:
  contextBrokers:
    - name: archive        # DNS label, at most 16 characters, neither main nor broker
      size: small          # sizes.yaml: small | medium | large
      storageClass: ''     # empty: storage.storageClass.rwo
```

Each one gets the release `context-broker-{name}`, its own CloudNativePG cluster
`context-broker-{name}-db` with the login `db-antares-{name}`, the admin token
`context-broker-{name}-admin-token`, a federated Keycloak client and audience
`context-broker-{name}`, a ServiceMonitor, and the NetworkPolicies of
`networkpolicies.yaml.gotmpl`: the gateway, the Portal and Prometheus reach the broker, and only
its own broker reaches its database (`policies/brokers.rego`). The gateway and the Portal find it
through `JC_GATEWAY_BROKER_URL_TEMPLATE` and `JC_PORTAL_BROKER_URL_TEMPLATE`.

With the Linkerd mesh on, every broker also gets a Linkerd `Server` on 9090 with
`accessPolicy: deny` and an `AuthorizationPolicy` that admits the `context-gateway` and `portal`
ServiceAccounts alone (T-4875). A meshed pod's connection lands on the proxy port the NetworkPolicy
opens, so the proxy refuses a forged subject from any other pod. Behind it,
`ANTARES_POLICY_SUBJECT_FROM` names the same two mesh identities: the broker believes the `x-jc-*`
subject headers from them alone, so an identity the Server admits for another reason, the broker's
own, names nobody and gets 401 without the admin bearer. `policies/brokers.rego` refuses a broker
`MeshTLSAuthentication` that names anyone else; Prometheus cannot scrape a meshed broker until its
metrics move to a port of their own.
