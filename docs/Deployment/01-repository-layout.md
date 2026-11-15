
# Repository layout

This page is the contract for the deployment code in this repository (`deploy/`) and the private environment values beside it (`environments/`), for the operator who renders or applies an environment and for every agent or person who pushes to this repository. This repository is public: its commits, branches and CI logs are readable by anyone, so a value that names a real server, key or person never enters it.

## 1. The two trees

The deployment code lives in `deploy/` of this repository. The values of the real environments live in `environments/` of a private repository, checked out next to it.

```text
deploy/                         public, this repository
  helmfile-root.yaml.gotmpl     the root helmfile
  helmfile-components.yaml.gotmpl
  justfile                      render, apply, smoke and guard recipes
  charts/ components/ defaults/ policies/ gitops/ images/ seeds/ scripts/ tests/
  cluster/hetzner/module/       the terraform module only, no *.tfvars
  ci/example-deployments/       the environment list and the PUBLIC example environments
    environments/               _layers addons broker-only local minimal production staging ...
  ci/golden/                    goldens of the public examples only
  rehearsal/                    the single-host form

environments/                   private repository, never public
  .sops.yaml                    creation rules and age recipients
  jc-dev/ jc-prod/ smartcity/ stage/ dev/    the real environments' values
  nodes.yaml                    the node address of each cluster the guards compare
  secrets/                      sops-encrypted files a seed names (encryptedSeedFiles)
  clusters/*.tfvars             the terraform variables of each cluster
  golden/                       goldens of the private environments
  tests/                        the tests that assert real values
  tools/render-parity.py        full-manifest render parity of two checkouts
```

A private environment's directory has the same shape as a public example's: `global.yaml.gotmpl` and one file per component. Release notes and the changelog of the old deployment repository stay in the private repository.

## 2. The one setting: `JC_ENVIRONMENTS`

`JC_ENVIRONMENTS` names the private `environments/` checkout as an absolute path. Nothing else points at private values.

```bash
export JC_ENVIRONMENTS=/srv/jc/environments   # a clean checkout of the private repository at a recorded sha
cd deploy && just dev-apply
```

`just _dev-assemble` copies the public examples into `deployment/environments/`, then the private overlay from `JC_ENVIRONMENTS` on top of them:

1. A private environment name rendered without `JC_ENVIRONMENTS` stops before helmfile runs, with `environment jc-dev is private: set JC_ENVIRONMENTS to the environments/ checkout`.
2. A private directory that also exists as a public example is refused: an overlay never shadows a public example silently.
3. The guards read each cluster's node address from `$JC_ENVIRONMENTS/nodes.yaml` (`just _node-ip <cluster>`). There is no default address: without the file a guard stops with `no node address for <cluster>: <path>/nodes.yaml is missing; set JC_ENVIRONMENTS to the environments/ checkout`.
4. A component default never holds a private value. The backup dumps' `ageRecipients` default is empty and the chart stops a render that enables a dump without them; each private environment sets its own in `global.yaml.gotmpl`.
5. A sops-encrypted seed file is added by the environment, `encryptedSeedFiles: {<repository path>: <file text>}`, read from `$JC_ENVIRONMENTS/secrets/`; a public seed folder holds no `*.enc.yaml`.

The deployer renders from a clean clone of the private repository at a recorded sha, never from a working tree, and records that sha beside the code commit, so a render is reproducible from two shas.

Public CI never checks out `environments/`. The private environments are rendered and tested on the host (the branch check and the deployer's apply), never in a GitHub Actions log of this repository.

## 3. What may never be public

| Class | Examples of the class | Where it lives instead | Public stand-in in tests |
|---|---|---|---|
| Server and node addresses | a node's IPv4, a firewall allow-list entry | `environments/nodes.yaml`, `environments/clusters/*.tfvars` | TEST-NET `192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24` |
| age recipients | `age1` followed by 58 characters | `environments/.sops.yaml`, each environment's `global.yaml.gotmpl` | a recipient the test generates in its temporary directory |
| sops content | an `ENC[...]` value, a `sops:` mapping, a `*.enc.yaml` file | `environments/secrets/` | none; a test asserts a public seed holds no sops file |
| terraform variables | any `*.tfvars` | `environments/clusters/` | none |
| Personal e-mail addresses | an owner's or a person's own address | the environment's values | `owner@example.org`, `security@example.org` |
| Private commit shas | fingerprints of the old private history, a `.gitleaksignore` entry not in this repository's history | `environments/gitleaksignore.private` | none |

Public: our own public host names under `joinedcontext.com`, environment names, the public services `1.1.1.1`, `1.0.0.1`, `8.8.8.8`, `9.9.9.9` and GitHub Pages, published open-data and synthetic demo contacts. A test that needs a real value lives in `environments/tests/`, which imports the helpers of `deploy/tests/` through `PYTHONPATH`; `deploy/tests/` has no skip for a missing private tree.

## 4. The public check

The public check refuses a push that carries any class of section 3. It runs before anything reaches GitHub, because CI runs on every `agent/**` push of this public repository, and a check that fails after the push has already published the value; a force-push does not take it back.

- Where it runs: the board's push tool and merge tool run it for every push to this repository, on the outgoing commits (every commit no `origin` ref holds; the merge tool checks the branch against `main`). CI runs the same rules as a second line.
- What it reads: every line a commit adds (for a merge, what the merge itself adds) and every commit message.
- What it refuses: a global IPv4 address outside its allow-list (TEST-NET and private ranges are no global address; a node address of `nodes.yaml` is refused even when allow-listed), an age recipient, a sops value or mapping, a `*.tfvars` file, a private e-mail address, a sha of the private history, a `.gitleaksignore` sha that is no commit of this repository; then `gitleaks git` on the same range.
- What it prints: the commit, the file and the line, and the rule. Never the matched value.
- It only refuses. It rewrites nothing and pushes nothing.

A refused push is fixed in the unpushed commits themselves (amend or rebase the local commits); a new commit on top leaves the value in the history. A dotted number that is no address, such as a specification clause, goes into the check's allow-list with its reason. A value that already reached GitHub is reported to the owner at once: the commit is public and stays readable until the owner removes it.

## Related

- [deploy/README.md](../../deploy/README.md): the single-host form and the cluster form of the deployment.
- [docs index](../README.md): the other pages of this repository's documentation.
