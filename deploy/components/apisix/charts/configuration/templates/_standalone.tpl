{{- /*
  The standalone APISIX rule file, indented to sit under a `apisix.yaml: |` key. Every component
  contributes routes and plugin configs through components/<c>/apisix-routes.yaml and
  apisix-plugins.yaml. The `#END` marker is required by APISIX standalone mode.
*/}}
{{- define "configuration.standalone" }}
    plugin_configs:
{{- range $id, $config := .Values.plugins }}
      - id: {{ $id }}
        desc: {{ $config.description | default $config.name | quote }}
        {{- /* The Portal sets this chain's limit-count to the Organization's rate for the class
               when it composes the served file (ADR-N-035, T-2892). */}}
        {{- with $config.rateClass }}
        labels:
          jc-rate-class: {{ . | quote }}
        {{- end }}
        plugins:
{{/* ${DOMAIN_RE} is the domain with its dots escaped, for a plugin that matches an Origin
     with a regular expression: an unescaped dot matches any character, so `.+\.${DOMAIN}`
     also admits a look-alike host somebody else can register (T-1679). */}}
{{ ($config.plugins | default dict) | toYaml | replace "${REALM}" $.Values.realm | replace "${DOMAIN_RE}" ($.Values.domain | replace "." "[.]") | replace "${DOMAIN}" $.Values.domain | replace "${APISIX_GATEWAY_CLIENT_SECRET}" "${{APISIX_GATEWAY_CLIENT_SECRET}}" | replace "${OIDC_SESSION_SECRET}" "${{OIDC_SESSION_SECRET}}" | replace "${EDGE_CLIENT_SECRET}" "${{EDGE_CLIENT_SECRET}}" | indent 10 }}
{{- end }}
    {{- /* Per request, `client-control` replaces the server block's ceiling in either direction
           (measured on the pinned image, tests/test_apisix_429_contract.py). */}}
    global_rules:
      - id: edge-body
        plugins:
          client-control:
            max_body_size: {{ mul (int .Values.maxRequestBodyMegabytes) 1048576 }}
    {{- /* T-5119: an address no route serves. APISIX runs the global rules and then writes
           `{"error_msg":"404 Route Not Found"}` itself; a bare 404 here instead lands on the
           server block's error page (a page or Problem Details, like every other edge error).
           T-5594: the same function keeps the edge's own session cookies off every upstream hop.
           `openid-connect` reads them in the route's rewrite phase, which runs before this access
           phase, and hands the upstream the person's token instead; no upstream reads them, and the
           Portal hop answered 431 to a steward's two-chunk 8 KB session beside its token. The names
           are the edge's (`__Host-jc_edge`, its chunks `__Host-jc_edge2`.., `__Host-jc_edge_apps`,
           `__Host-jc_edge_app_{name}`) and an App host's (`__Host-jc_app_{name}`); every other
           cookie, the Portal's `__Host-jc_session` and `__Host-jc_csrf` among them, passes. One
           rule holds both, because APISIX drops a plugin two global rules name. T-6170: it also
           holds the context of a request on a route with a `limit-conn` under `$request_id`, so
           the server block's log phase can give the slot back when the answer ends in one of the
           edge's error pages (an `error_page` redirect empties `ngx.ctx`); `edge-headers` drops
           the entry when the route answers itself. Bounded (20 000 per worker, 15 min). */}}
      - id: edge-not-found
        plugins:
          serverless-pre-function:
            phase: access
            functions:
              - >-
                return function(conf, ctx)
                  if ctx.matched_route == nil then
                    return ngx.exit(404)
                  end
                  local plugins = ctx.matched_route.value.plugins
                  if plugins and plugins["limit-conn"] then
                    local held = package.loaded["jc.edge.held"]
                    if not held then
                      held = require("resty.lrucache").new(20000)
                      package.loaded["jc.edge.held"] = held
                    end
                    held:set(ngx.var.request_id, ctx, 900)
                  end
                  local raw = ngx.req.get_headers()["Cookie"]
                  if raw == nil then
                    return
                  end
                  if type(raw) == "table" then
                    raw = table.concat(raw, "; ")
                  end
                  local kept, dropped = {}, false
                  for pair in raw:gmatch("[^;]+") do
                    local cookie = pair:match("^%s*(.-)%s*$")
                    if cookie:find("^__Host%-jc_edge") or cookie:find("^__Host%-jc_app_") then
                      dropped = true
                    elseif cookie ~= "" then
                      kept[#kept + 1] = cookie
                    end
                  end
                  if dropped then
                    require("apisix.core").request.set_header(ctx, "Cookie", #kept > 0 and table.concat(kept, "; ") or nil)
                  end
                end
    {{- /* T-5107 (ZAP baseline): the floor of every answer, the edge's own 404 for an address no
           route serves included, added only where nothing upstream or on the route set it. An
           App on its own host chooses its own Permissions-Policy (a map App asks for location). */}}
      - id: edge-headers
        plugins:
          serverless-post-function:
            phase: header_filter
            functions:
              - >-
                return function()
                  local held = package.loaded["jc.edge.held"]
                  if held then
                    held:delete(ngx.var.request_id)
                  end
                  local h = ngx.header
                  if h["Strict-Transport-Security"] == nil then
                    h["Strict-Transport-Security"] = "max-age=31536000; includeSubDomains; preload"
                  end
                  if h["X-Content-Type-Options"] == nil then
                    h["X-Content-Type-Options"] = "nosniff"
                  end
                  local host = ngx.var.host or ""
                  local apps = ".apps.{{ .Values.domain }}"
                  if h["Permissions-Policy"] == nil and host:sub(-#apps) ~= apps then
                    h["Permissions-Policy"] = "camera=(), microphone=(), geolocation=(), payment=(), usb=()"
                  end
                end
    upstreams:
{{- range $id, $config := .Values.routes }}
      - id: {{ $id }}
        type: roundrobin
        {{- if hasKey $config "scheme" }}
        scheme: {{ $config.scheme }}
        pass_host: {{ $config.passHost | default "pass" }}
        {{- end }}
        nodes:
          {{ $config.upstream | quote }}: 1
        timeout:
          connect: {{ ($config.timeout | default dict).connect | default 6 }}
          send: {{ ($config.timeout | default dict).send | default 30 }}
          read: {{ ($config.timeout | default dict).read | default 30 }}
        {{- with $config.keepalivePool }}
        keepalive_pool:
          size: {{ .size | default 320 }}
          idle_timeout: {{ .idleTimeout | default 60 }}
          requests: {{ .requests | default 1000 }}
        {{- end }}
{{- end }}
    routes:
{{- range $id, $config := .Values.routes }}
      - id: {{ $id }}
        name: {{ $config.name | quote }}
        desc: {{ $config.description | default $config.name | quote }}
        uri: {{ $config.uri | quote }}
        {{- if hasKey $config "priority" }}
        priority: {{ $config.priority }}
        {{- end }}
        {{- if hasKey $config "methods" }}
        methods: {{ $config.methods | toJson }}
        {{- end }}
        {{- if hasKey $config "vars" }}
        vars: {{ $config.vars | toJson }}
        {{- end }}
        {{- /* The upgrade is passed on only where the route says so (T-3905): the gateway
               authenticates before it answers 101 (platform T-3885). */}}
        {{- if $config.websocket }}
        enable_websocket: true
        {{- end }}
        {{- if hasKey $config "subDomain" }}
        host: {{ printf "%s.%s" $config.subDomain $.Values.domain | quote }}
        {{- else if hasKey $config "hosts" }}
        hosts: {{ $config.hosts | toJson }}
        {{- else if hasKey $config "host" }}
        host: {{ $config.host | quote }}
        {{- end }}
        upstream_id: {{ $id }}
        {{- $pluginConfig := $config.pluginConfig | default $id }}
        {{- if hasKey $.Values.plugins $pluginConfig }}
        plugin_config_id: {{ $pluginConfig }}
        {{- end }}
        {{- with $config.rateClass }}
        labels:
          jc-rate-class: {{ . | quote }}
        {{- end }}
        {{- /* A route-level plugin replaces the same plugin of its plugin config: the one way a
               route shares a chain and counts a bucket of its own (T-2892). */}}
        {{- with $config.plugins }}
        plugins:
{{ toYaml . | replace "${DOMAIN_RE}" ($.Values.domain | replace "." "[.]") | indent 10 }}
        {{- end }}
{{- end }}
    #END
{{- end }}
