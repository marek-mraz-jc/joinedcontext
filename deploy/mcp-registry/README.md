# MCP Registry listing

`server.json` lists the public open-data connector in the official MCP Registry
(<https://registry.modelcontextprotocol.io>, docs <https://modelcontextprotocol.io/registry/about>), which
MCP clients and other directories read. The registry stores only this metadata; the connector stays
`https://api.smartcity.joinedcontext.com/api/mcp`, read-only and anonymous. `server.schema.json` is the
registry's schema of 2025-12-11, vendored so `tests/test_mcp_registry.py` checks the file offline.

The name `com.joinedcontext/open-data` needs DNS proof for `joinedcontext.com` (registry docs,
Authentication, DNS). The owner publishes it, because the key and the DNS record are theirs:

1. `openssl genpkey -algorithm Ed25519 -out key.pem` outside any repo, kept with the owner's secrets.
2. Add the TXT record at the DNS provider for `joinedcontext.com.`:
   `v=MCPv1; k=ed25519; p=<openssl pkey -in key.pem -pubout -outform DER | tail -c 32 | base64>`
3. From this folder: `mcp-publisher login dns --domain joinedcontext.com --private-key <hex from
   openssl pkey -in key.pem -noout -text>` then `mcp-publisher publish`.
4. Check: `curl 'https://registry.modelcontextprotocol.io/v0.1/servers?search=com.joinedcontext/open-data'`.

A later change (new URL, title, icon) bumps `version` and publishes again; the registry refuses a
version it already holds.
