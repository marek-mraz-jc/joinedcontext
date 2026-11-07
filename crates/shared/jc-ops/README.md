# jc-ops

The operation registry. A module declares each operation once (name, input, output, the permission it checks); REST and OData
(jc-odata), MCP tools (jc-mcp) and the CLI (`jc <module> <operation>`, replacing jcctl) are generated from it, so a person in the
UI, a script and an AI agent reach the same operations with the same permissions and organization scope.
