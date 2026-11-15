"""T-2475: an application's repository may be copied to GitHub as well (AP-79).

The Portal reads `JC_APP_MIRROR_GITHUB_OWNER` and `JC_APP_MIRROR_GITHUB_TOKEN` together or not at
all, and refuses to start with one of them. So the chart sets both or neither, the token is a
`secretKeyRef` to an operator-supplied Secret and never a value, and an installation that names no
owner asks the pod for no Secret, which would otherwise hold it in CreateContainerConfigError.
"""




def portal_env(docs):
    portal = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal")
    return {e["name"]: e for e in portal["spec"]["template"]["spec"]["containers"][0].get("env", [])}


def with_owner(owner):
    def edit(tree):
        path = tree / "components/portal/default-environment.yaml.gotmpl"
        text = path.read_text()
        assert "githubOwner: ''" in text
        path.write_text(text.replace("githubOwner: ''", f"githubOwner: {owner}"))

    return edit


def test_no_owner_means_no_copy_and_no_secret_asked_for(rendered):
    env = portal_env(rendered("dev"))
    assert "JC_APP_MIRROR_GITHUB_OWNER" not in env
    assert "JC_APP_MIRROR_GITHUB_TOKEN" not in env


def test_an_owner_sets_both_variables_and_the_token_comes_from_the_secret(rendered_variant):
    env = portal_env(rendered_variant("dev", with_owner("hel-apps")))
    assert env["JC_APP_MIRROR_GITHUB_OWNER"]["value"] == "hel-apps"
    token = env["JC_APP_MIRROR_GITHUB_TOKEN"]
    assert "value" not in token, "a token in the manifest is a token in Git"
    assert token["valueFrom"]["secretKeyRef"] == {"name": "github-app-mirror", "key": "token"}
