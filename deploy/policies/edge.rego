# The edge writes two answers of its own, and one of them had no shape: every `limit-count`
# rejects with `429` and APISIX serves its own HTML page unless the server block turns that
# status into Problem Details (T-2237, API-08, GW26). The platform's error contract is RFC 9457
# and ETSI CIM 009 requires it on an error of the NGSI-LD routes, so a rendered edge whose server
# block lost the handler is a rendered edge that breaks the contract for the one status the edge
# alone produces.
package main

edge_server_block(body) if {
	contains(body, "http_server_configuration_snippet")
}

deny contains msg if {
	input.kind == "ConfigMap"
	some key, body in input.data
	edge_server_block(body)
	not contains(body, "error_page 429")
	msg := sprintf(
		"ConfigMap/%s: %s carries the edge server block without `error_page 429`, so a rate-limited caller gets an HTML page instead of application/problem+json (T-2237, API-08)",
		[input.metadata.name, key],
	)
}

deny contains msg if {
	input.kind == "ConfigMap"
	some key, body in input.data
	edge_server_block(body)
	contains(body, "error_page 429")
	not contains(body, "default_type application/problem+json")
	msg := sprintf(
		"ConfigMap/%s: %s answers a 429 from the server block without the problem+json media type (T-2237, API-08)",
		[input.metadata.name, key],
	)
}

deny contains msg if {
	input.kind == "ConfigMap"
	some key, body in input.data
	edge_server_block(body)
	contains(body, "error_page 429")
	not contains(body, "add_header Retry-After")
	msg := sprintf(
		"ConfigMap/%s: %s refuses with 429 and tells the client nothing about when to come back (T-2237, GW26)",
		[input.metadata.name, key],
	)
}
