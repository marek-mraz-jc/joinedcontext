# Reading the configuration with git

Task T-6896 (T-6893 part 2). Every configuration write ends as a commit on the forge's `main`. A person who may
read a repository clones it with git over HTTPS, read-only, at the `cloneUrl` that `jc_repository_get` answers
(`https://{domain}/git/{owner}/{repo}.git`).

## 1. Cloning

git sends the person's Keycloak access token in the `Authorization` header:

```sh
git -c http.extraHeader="Authorization: Bearer $TOKEN" clone https://smartcity.example/git/joinedcontext/configuration.git
```

A token in the URL (`?access_token=...`, `https://user:token@...`) is refused: a URL ends up in logs and shell
history. The token is an access token the realm issued for the Portal, the one the Portal's MCP server takes.

## 2. Who reads what

| Repository | Who clones it |
|---|---|
| The installation repository (`joinedcontext/configuration`), which holds every Organization | platform administrators: the bootstrap group (`platform-admins`) or a binding at organization scope with `approve` and `delete` on RoleBinding and no Organization narrowing it. An Organization's administrator does not. |
| A project repository (`joinedcontext/<project>-config`) | the members of that project: any binding in force that reaches it |
| Any other repository, an App's included | nobody |

## 3. What the door refuses

The door is the Portal's (`crates/portal/src/forge/git_door.rs`). It takes two requests and no third:
`GET /git/{owner}/{repo}.git/info/refs?service=git-upload-pack` and `POST /git/{owner}/{repo}.git/git-upload-pack`.

- Every refusal answers the same `404`: no token, a bad or revoked token, no right, a repository that is not
  there, a push (`git-receive-pack`), any other `/git` path. The answer never tells which repositories exist.
- One person makes at most 60 requests a minute; a clone or a fetch is two. Past that, the same `404` for the
  rest of the minute.
- An upload-pack request (its wants and haves) is read up to 4 MiB, gzip inflated up to the same size.
- The Portal reads the forge with its own machine user, inside the pod, and streams the pack back.

## 4. Where it is open

- The APISIX edge (joinedcontext-deployment `components/jc/apisix-plugins.yaml`) blocks `/git` except the
  two paths above, spelt exactly.
- jc in host mode (jc-dev, jc-prod) answers its public `/git` mount with the door alone and keeps the forge for
  the Portal on the in-pod listener. In slug mode the Portal dials the forge on jc's one listener, so `/git`
  there stays the forge's own, which takes the forge's tokens alone.
