"""T-2743 (UI-82, PF-90, AP-111): the realm's login pages are the `joinedcontext` theme.

The theme is a ConfigMap of FreeMarker, CSS and messages (components/keycloak/charts/theme)
that the Keycloak pod copies into its themes directory. What a person reads there, the app
and the organization, comes from the client's name and the realm's display name; the
colours and the logo come from the branding block.
"""

import base64
import json
import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

import pytest

PROJECT_ROOT = Path(__file__).resolve().parent.parent
THEME = PROJECT_ROOT / "components/keycloak/charts/theme"
LOCALES = ("en", "sk", "cs", "de")


def realm_of(docs):
    for doc in docs:
        if doc.get("kind") == "Secret" and doc["metadata"]["name"].endswith("config-cli-config-realms"):
            (payload,) = [v for k, v in doc["data"].items() if k != "master.json"]
            return json.loads(base64.b64decode(payload))
    pytest.fail("no keycloak-config-cli realm Secret in the render")


def theme_of(docs):
    for doc in docs:
        if doc.get("kind") == "ConfigMap" and doc["metadata"]["name"] == "keycloak-theme":
            return doc
    pytest.fail("no keycloak-theme ConfigMap in the render")


def messages(data, locale):
    text = data[f"login__messages__messages_{locale}.properties"]
    return dict(
        line.split("=", 1) for line in text.splitlines() if line and not line.startswith("#")
    )


@pytest.mark.parametrize("env", ["local", "dev"])
def test_the_realm_signs_people_in_on_the_joinedcontext_theme(rendered, env):
    realm = realm_of(rendered(env))
    assert realm["loginTheme"] == "joinedcontext"
    # The organization is named, never the realm id.
    assert realm["displayName"] and realm["displayName"] != realm["realm"]
    assert set(LOCALES) <= set(realm["supportedLocales"]) and realm["internationalizationEnabled"]


def test_the_dev_realm_is_named_after_the_organization_of_the_branding_block(rendered):
    assert realm_of(rendered("dev"))["displayName"] == "joinedcontext demo"


def test_the_theme_extends_keycloak_v2_with_the_branding_colours_and_logo(rendered):
    data = theme_of(rendered("dev"))["data"]
    properties = dict(line.split("=", 1) for line in data["login__theme.properties"].splitlines())
    assert properties["parent"] == "keycloak.v2"
    assert properties["locales"] == ",".join(LOCALES)
    assert properties["darkMode"] == "false", "the branding is one light palette"
    assert properties["jcLogo"] == "logo.svg" and data["login__resources__img__logo.svg"].startswith("<svg")
    css = data["login__resources__css__branding.css"]
    assert "--jc-primary: #0000bf;" in css and "--jc-primary-fg: #ffffff;" in css


def test_every_message_the_theme_adds_is_in_all_four_languages(rendered):
    data = theme_of(rendered("local"))["data"]
    keys = {locale: set(messages(data, locale)) for locale in LOCALES}
    for locale in LOCALES[1:]:
        assert keys[locale] == keys["en"], (locale, keys["en"] ^ keys[locale])


def test_a_client_a_person_signs_into_is_named_with_a_key_the_theme_translates(rendered):
    """AP-111: the login header reads "Sign in to {client name} · {organization}", so a
    browser client's name is a `${jcClient…}` key, never an internal label like "Edge login"."""
    docs = rendered("dev")
    data = theme_of(docs)["data"]
    names = {c["clientId"]: c["name"] for c in realm_of(docs)["clients"]}
    for client_id in ("edge", "portal-ui", "portal-api", "ckan", "gitea"):
        name = re.fullmatch(r"\$\{(jcClient\w+)\}", names[client_id])
        assert name, (client_id, names[client_id])
        for locale in LOCALES:
            assert messages(data, locale).get(name.group(1)), (client_id, locale)


def test_the_keycloak_pod_copies_the_theme_in_and_rolls_when_it_changes(rendered):
    docs = rendered("local")
    (keycloak,) = [
        d for d in docs if d.get("kind") == "StatefulSet" and "keycloak" in d["metadata"]["name"]
    ]
    pod = keycloak["spec"]["template"]
    assert re.fullmatch(r"[0-9a-f]{64}", pod["metadata"]["annotations"]["checksum/theme"])
    volumes = {v["name"]: v for v in pod["spec"]["volumes"]}
    assert volumes["theme"]["configMap"]["name"] == "keycloak-theme"
    (build,) = [c for c in pod["spec"]["initContainers"] if c["name"] == "build-keycloak"]
    assert {"name": "theme", "mountPath": "/theme", "readOnly": True} in build["volumeMounts"]
    assert "/app/themes/joinedcontext/" in build["args"][0]
    # The copy precedes the build, and a failed copy stops the pod instead of starting Keycloak
    # on the stock pages.
    script = build["args"][0]
    assert script.index("set -eu") < script.index("/theme/*") < script.index("kc.sh build")


def test_the_theme_loads_nothing_from_another_origin_and_adds_no_script():
    """CSP unchanged or stricter: no external asset anywhere, and the pages the theme writes
    carry no script and no inline handler. login's template.ftl and login.ftl and the account
    console's index.ftl are Keycloak's own pages with one change each, their scripts kept."""
    for path in (THEME / "files").iterdir():
        text = path.read_text()
        assert not re.search(r"(src|href)=[\"']?(https?:)?//|url\(|@import", text), path.name
        if path.name not in ("login__template.ftl", "login__login.ftl", "account__index.ftl"):
            assert not re.search(r"<script|\son[a-z]+=", text), path.name
    template = (THEME / "files/login__template.ftl").read_text()
    assert "prefers-color-scheme" not in template, "the dark-mode script is gone"


def helm_template(branding):
    return subprocess.run(
        ["helm", "template", "t", str(THEME), "--set-json", "branding=" + json.dumps(branding)],
        capture_output=True,
        text=True,
    )


@pytest.mark.parametrize(
    "branding",
    [
        {"colours": {"primary": "red;} body { display: none"}},
        {"fonts": {"body": "x; background: url(//evil.example)"}},
        {"logo": "../logo.svg", "logoSvg": "<svg/>"},
        {"logo": "logo.svg", "logoSvg": '<svg onload="alert(1)"/>'},
        {"logo": "logo.svg", "logoSvg": "<svg><script>alert(1)</script></svg>"},
        # F-R2DEP2-004: the event attribute after a slash or a quote, a reference that spells
        # `javascript:`, an entity declared in the file, and a link out of the drawing.
        {"logo": "logo.svg", "logoSvg": "<svg/onload=alert(1)>"},
        {"logo": "logo.svg", "logoSvg": '<svg><g id="a"/onclick="alert(1)"></g></svg>'},
        {"logo": "logo.svg", "logoSvg": '<svg><a href="javascript&#58;alert(1)"><path/></a></svg>'},
        {"logo": "logo.svg", "logoSvg": '<!DOCTYPE svg [<!ENTITY x "x">]><svg>&x;</svg>'},
        {"logo": "logo.svg", "logoSvg": '<svg><use href="https://evil.example/x.svg#a"/></svg>'},
        {"logo": "logo.svg", "logoSvg": '<svg><image xlink:href="data:image/svg+xml;base64,PHN2Zz4="/></svg>'},
    ],
)
def test_a_branding_value_that_is_not_a_colour_font_or_drawing_stops_the_render(branding):
    result = helm_template(branding)
    assert result.returncode != 0, result.stdout
    assert "branding." in result.stderr


@pytest.mark.parametrize("logo", sorted((PROJECT_ROOT / "ci/example-deployments/environments").glob("*/branding/logo.svg")))
def test_a_plain_drawing_with_a_local_reference_renders(logo):
    """The guard refuses what runs, not what draws: the shipped logos and a gradient that
    points at a `#fragment` of its own file still render."""
    for svg in (logo.read_text(), '<svg><linearGradient id="g"/><use href="#g"/><rect fill="url(#g)"/></svg>'):
        result = helm_template({"logo": "logo.svg", "logoSvg": svg})
        assert result.returncode == 0, result.stderr


def test_a_light_primary_colour_gets_dark_button_text():
    """WCAG 1.4.3: white on a pale primary would not read, so the chart picks the text colour."""
    result = helm_template({"colours": {"primary": "#ffe977"}})
    assert result.returncode == 0, result.stderr
    assert "--jc-primary-fg: #000000;" in result.stdout


def test_keycloak_is_built_with_script_mappers_off_and_nothing_turns_them_on(rendered):
    """T-2857: a script mapper runs code in every token its client mints, so managing an App's
    client would be minting any claim. The build disables the feature by name, and no
    environment variable of the pod enables it again."""
    docs = rendered("local")
    (keycloak,) = [
        d for d in docs if d.get("kind") == "StatefulSet" and "keycloak" in d["metadata"]["name"]
    ]
    pod = keycloak["spec"]["template"]["spec"]
    (build,) = [c for c in pod["initContainers"] if c["name"] == "build-keycloak"]
    (line,) = [line for line in build["args"][0].splitlines() if "kc.sh build" in line]
    assert "--features-disabled=scripts" in line.split()
    # T-5954: CIMD (experimental) and parameterized scopes (preview) for the connector sign-in;
    # resource indicators stay off (exact-match static list, breaks today's connectors).
    assert "--features=cimd,parameterized-scopes" in line.split()
    assert "resource-indicators" not in line
    assert not re.search(r"--features(=|\s)\S*scripts", line)
    for container in pod["initContainers"] + pod["containers"]:
        for env in container.get("env", []):
            if env["name"] in ("KC_FEATURES", "KC_FEATURES_DISABLED"):
                assert "scripts" not in env.get("value", "").split(",") or env["name"] == "KC_FEATURES_DISABLED", env



def test_the_account_console_wears_the_portal_and_leads_back_to_it(rendered):
    """T-4203: the account console is keycloak.v3 with the branding logo and colours, no dark
    mode, and its logo and "Back to …" line lead to the Portal of this domain."""
    assert realm_of(rendered("dev"))["accountTheme"] == "joinedcontext"
    data = theme_of(rendered("dev"))["data"]
    properties = dict(line.split("=", 1) for line in data["account__theme.properties"].splitlines())
    assert properties["parent"] == "keycloak.v3"
    assert properties["darkMode"] == "false"
    assert properties["logo"] == "img/logo.svg" and data["account__resources__img__logo.svg"].startswith("<svg")
    assert properties["logoUrl"] == properties["jcPortalUrl"] == "https://portal.dev.joinedcontext.com"
    assert "--pf-v5-global--primary-color--100: #0000bf;" in data["account__resources__css__branding.css"]
    index = data["account__index.ftl"]
    assert '"referrerUrl": "${jcReferrerUrl}"' in index and "properties.jcPortalUrl" in index


def test_a_portal_url_that_is_not_an_https_origin_stops_the_render():
    for url in ["http://portal.example", "https://portal.example/x\" onload=1", "javascript:alert(1)"]:
        result = subprocess.run(
            ["helm", "template", "t", str(THEME), "--set-string", f"portalUrl={url}"],
            capture_output=True, text=True,
        )
        assert result.returncode != 0 and "portalUrl" in result.stderr, url


def test_a_wrong_password_is_the_forms_alert_and_the_card_names_the_app_alone():
    """T-4203: either value can be wrong, so no field is marked; the organization is in the
    brand header already, so the card's heading does not repeat it (four lines on a phone)."""
    login = (THEME / "files/login__login.ftl").read_text()
    assert "displayMessage=true" in login and 'name="username" label=label error=""' in login
    assert "getFirstError('username','password')" not in login
    template = (THEME / "files/login__template.ftl").read_text()
    assert '${msg("jcSignInToApp", app)}' in template
    (heading,) = [line for line in template.splitlines() if 'id="kc-page-title"' in line]
    assert "org" not in heading


def test_the_account_console_speaks_slovak_with_keys_keycloak_knows(rendered):
    """T-4228: keycloak.v3 ships no Slovak for most account console strings; the theme's
    messages_sk fills the ones a person sees. Every key is one the 26.8 bundle has (a typo
    would stay English silently), every value is filled, and a placeholder survives."""
    text = theme_of(rendered("dev"))["data"]["account__messages__messages_sk.properties"]
    sk = dict(line.split("=", 1) for line in text.splitlines() if line and not line.startswith("#"))
    fixture = PROJECT_ROOT / "tests/fixtures/keycloak-account-v3-keys.txt"
    known = {line for line in fixture.read_text().splitlines() if line and not line.startswith("#")}
    assert set(sk) <= known, set(sk) - known
    assert all(value.strip() for value in sk.values())
    for key in ("personalInfo", "selectALocale", "choose", "jumpToSection", "general", "signingIn"):
        assert key in sk, key
    assert sk["error-invalid-length"].count("{{") == 3


RENDER_TITLE = """\
import freemarker.core.HTMLOutputFormat;
import freemarker.template.Configuration;
import freemarker.template.Template;
import java.io.StringReader;
import java.io.StringWriter;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;

public class RenderTitle {
    public static void main(String[] args) throws Exception {
        Configuration cfg = new Configuration(Configuration.VERSION_2_3_32);
        cfg.setOutputFormat(HTMLOutputFormat.INSTANCE);
        Template page = new Template("page", new StringReader(Files.readString(Path.of(args[0]))), cfg);
        StringWriter out = new StringWriter();
        page.process(Map.of("client", args[1]), out);
        System.out.print(out);
    }
}
"""


def title_of(header, client):
    """Render the theme's own title lines under Keycloak's FreeMarker (HTML output format)
    for a page whose header section is `header`."""
    template = (THEME / "files/login__template.ftl").read_text()
    start = template.index("<#assign titleText")
    end = template.index("</title>", start) + len("</title>")
    images = (PROJECT_ROOT / "components/keycloak/images.yaml").read_text()
    digest = re.search(r"digest: '(sha256:[0-9a-f]{64})'", images).group(1)
    page = (
        '<#macro layout><#assign signIn=false org="Banská Bystrica">'
        '<#assign header><#nested "header"></#assign>' + template[start:end] + "</#macro>"
        '<@layout; section><#if section == "header">' + header + "</#if></@layout>"
    )
    with tempfile.TemporaryDirectory() as work:
        Path(work, "RenderTitle.java").write_text(RENDER_TITLE)
        Path(work, "page.ftl").write_text(page)
        result = subprocess.run(
            ["docker", "run", "--rm", "--user", f"{os.getuid()}:{os.getgid()}",
             "-v", f"{work}:/work:ro", "--entrypoint", "java",
             f"quay.io/keycloak/keycloak@{digest}",
             "-cp", "/opt/keycloak/lib/lib/main/org.freemarker.freemarker-2.3.35.jar",
             "/work/RenderTitle.java", "/work/page.ftl", client],
            capture_output=True, text=True, timeout=300, check=False,
        )
    assert result.returncode == 0, result.stderr
    return result.stdout.strip()


@pytest.mark.skipif(shutil.which("docker") is None, reason="docker not installed")
def test_a_header_wrapped_in_an_element_gives_a_title_of_its_text_alone():
    """T-6306: keycloak.v2's consent page header is `<p>Grant Access to {client}</p>`, and the
    title element shows markup as text: the tab read "<p> Grant Access to Claude </p>"."""
    header = "<!-- template: login-oauth-grant.ftl -->\n<p>\n  Grant Access to ${client}\n</p>"
    assert title_of(header, "Claude") == "<title>Grant Access to Claude · Banská Bystrica</title>"


@pytest.mark.skipif(shutil.which("docker") is None, reason="docker not installed")
def test_a_client_name_with_markup_stays_text_in_the_title():
    """A client's name is whatever its registration says (CIMD): it reaches the title as
    escaped text and cannot open an element in the page head."""
    title = title_of("<p>Grant Access to ${client}</p>", "Evil</title><script>alert(1)</script> & co")
    assert title == (
        "<title>Grant Access to Evil&lt;/title&gt;&lt;script&gt;alert(1)&lt;/script&gt; &amp; co"
        " · Banská Bystrica</title>"
    )
