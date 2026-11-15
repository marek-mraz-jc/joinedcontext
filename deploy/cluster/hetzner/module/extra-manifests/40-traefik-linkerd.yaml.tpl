# Mesh the k3s-bundled Traefik: CIVITAS enforces cluster-authenticated inbound
# (mandatory mTLS), so the edge proxy must speak Linkerd mTLS. Traefik's own
# inbound stays open to the internet.
# skip-inbound-ports 8000,8443: without it the Linkerd inbound proxy makes
# Traefik see 127.0.0.1 as every peer (standard Linkerd ingress pattern);
# outbound mTLS to APISIX is untouched.
# The ports below only with mqtt_ports_open. mqtts 8883 (T-3996): the MQTT brokers' IngressRouteTCP entrypoint. It is in
# skip-inbound-ports too, or Traefik writes 127.0.0.1 into the PROXY v2 header
# and the gateway's CONNECT check takes a device for an in-cluster caller.
# mqtt-18830..mqtt-18839 (T-5856): one entrypoint per dedicated MQTT port, each
# routed by the Portal to the one broker holding it (HostSNI(*), TLS passthrough
# to RMQTT 8884 with PROXY v2); in skip-inbound-ports for the same reason. 18840-18849 stay closed.
# externalTrafficPolicy Local keeps the client address through svclb (T-0929).
# Probes and resources (T-5836): kube-hetzner's defaults (readiness 2 s with one failure, liveness 2 s x 3,
# CPU limit 300m) took both pods out of the Local LoadBalancer and then killed them together each time the
# node stalled (apiserver not ready, 2026-10-26T23:53Z, 2026-10-27T00:49Z, 03:48Z). The probes now ride out
# a 60 s stall, CPU gets 200m requested and a 1-core limit (was 300m: throttled under load), memory headroom.
apiVersion: helm.cattle.io/v1
kind: HelmChartConfig
metadata:
  name: traefik
  namespace: kube-system
spec:
  valuesContent: |-
    deployment:
      podAnnotations:
        linkerd.io/inject: enabled
        config.linkerd.io/skip-inbound-ports: "8000,8443%{ if mqtt_ports_open == "true" },8883,18830-18839%{ endif }"
%{ if mqtt_ports_open == "true" ~}
    ports:
      mqtts:
        port: 8883
        expose:
          default: true
        exposedPort: 8883
        protocol: TCP
      mqtt-18830:
        port: 18830
        expose:
          default: true
        exposedPort: 18830
        protocol: TCP
      mqtt-18831:
        port: 18831
        expose:
          default: true
        exposedPort: 18831
        protocol: TCP
      mqtt-18832:
        port: 18832
        expose:
          default: true
        exposedPort: 18832
        protocol: TCP
      mqtt-18833:
        port: 18833
        expose:
          default: true
        exposedPort: 18833
        protocol: TCP
      mqtt-18834:
        port: 18834
        expose:
          default: true
        exposedPort: 18834
        protocol: TCP
      mqtt-18835:
        port: 18835
        expose:
          default: true
        exposedPort: 18835
        protocol: TCP
      mqtt-18836:
        port: 18836
        expose:
          default: true
        exposedPort: 18836
        protocol: TCP
      mqtt-18837:
        port: 18837
        expose:
          default: true
        exposedPort: 18837
        protocol: TCP
      mqtt-18838:
        port: 18838
        expose:
          default: true
        exposedPort: 18838
        protocol: TCP
      mqtt-18839:
        port: 18839
        expose:
          default: true
        exposedPort: 18839
        protocol: TCP
%{ endif ~}
    service:
      spec:
        # Who the caller is (T-0929). k3s serves this LoadBalancer with svclb pods, and with
        # the default `Cluster` policy they SNAT: Traefik sees 10.42.0.1 for every request on
        # the internet, writes that into X-Real-Ip and X-Forwarded-For, and every address-keyed
        # decision downstream — the access log, APISIX's `limit-count`, the gateway's own
        # per-caller limiter — counts the whole internet as one caller. `Local` keeps the
        # source address; on a single-node cluster nothing becomes unreachable by it, because
        # the only node is the one running Traefik.
        externalTrafficPolicy: Local
    resources:
      requests:
        cpu: 200m
        memory: 64Mi
      limits:
        cpu: 1000m
        memory: 256Mi
    readinessProbe:
      timeoutSeconds: 5
      failureThreshold: 3
    livenessProbe:
      timeoutSeconds: 5
      failureThreshold: 6
