# NetworkPolicies Helm Chart

This Helm chart creates Kubernetes NetworkPolicy resources based on declarative configuration files located in each component.

## How it Works

Similar to the `databases` chart, this chart follows a declarative pattern:

1. Each component can have a `networkpolicies.yaml` file
2. The `defaults/environment/networkpolicies.yaml.gotmpl` template collects all `networkpolicies.yaml` files from enabled components
3. This chart receives the consolidated configuration and creates NetworkPolicy resources

## Component Configuration

Create a `networkpolicies.yaml` file in your component directory with the following structure:

```yaml
---
# Network policy name
my-service:
  # Pod selector - which pods this policy applies to
  podSelector:
    app.kubernetes.io/name: my-service

  # Policy types
  policyTypes:
    - Ingress
    # - Egress

  # Ingress rules
  ingress:
    - from:
        - namespaceSelector: {}
          podSelector:
            matchLabels:
              app.kubernetes.io/name: allowed-service

  # Optional: Egress rules
  # egress:
  #   - to:
  #       - namespaceSelector: {}
  #         podSelector:
  #           matchLabels:
  #             app.kubernetes.io/name: target-service

  # Optional: Custom labels
  # labels:
  #   custom-label: value

  # Optional: Annotations
  # annotations:
  #   custom-annotation: value
```

## Examples

### Simple Ingress Policy

```yaml
postgres:
  podSelector:
    app.kubernetes.io/name: postgres
  policyTypes:
    - Ingress
  ingress:
    - from:
        - namespaceSelector: {}
          podSelector:
            matchLabels:
              app.kubernetes.io/name: portal
```

### Policy with Egress Rules

```yaml
config-adapter:
  podSelector:
    app.kubernetes.io/name: config-adapter
  policyTypes:
    - Ingress
    - Egress
  ingress:
    - ports:
        - protocol: TCP
          port: 8080
  egress:
    - to:
        - namespaceSelector: {}
          podSelector:
            matchLabels:
              k8s-app: kube-dns
      ports:
        - protocol: UDP
          port: 53
```

### Multiple Policies per Component

You can define multiple network policies in a single file:

```yaml
portal-frontend:
  podSelector:
    app.kubernetes.io/name: portal
    app.kubernetes.io/component: frontend
  policyTypes:
    - Ingress
  ingress:
    - from:
        - namespaceSelector: {}
          podSelector:
            matchLabels:
              app.kubernetes.io/name: apisix

portal-backend:
  podSelector:
    app.kubernetes.io/name: portal
    app.kubernetes.io/component: backend
  policyTypes:
    - Ingress
  ingress:
    - from:
        - namespaceSelector: {}
          podSelector:
            matchLabels:
              app.kubernetes.io/name: apisix
```

## Benefits

- **Centralized Management**: All network policies are managed through component configuration files
- **Version Control**: Network policies are tracked alongside component code
- **Declarative**: Simple YAML structure that mirrors Kubernetes NetworkPolicy spec
- **Programmatic**: The helm chart handles the boilerplate, you only specify what's unique
- **Consistent**: Follows the same pattern as databases, apisix-routes, etc.

## Comparison to Individual NetworkPolicy Charts

**Before** (old pattern):

- Each component had its own networkpolicy chart in `components/<component>/charts/<component>-networkpolicy/`
- Values had to be passed through helmfile
- More boilerplate and duplication

**After** (new pattern):

- Single networkpolicies chart manages all policies
- Simple `networkpolicies.yaml` file per component
- Less boilerplate, easier to maintain
- Consistent with other declarative patterns in the project
