"""Robot listener: fixtures outside the view (surfaces-conformance.md 2.3, T-3501).

A TP's setup and teardown, its suite's too, reach the plain root given as the
listener's first argument; its body reaches the URL under test (`url` and
`temporal_api_url` of variables.py, a view's NGSI-LD root). A view refuses
the writes most setups make, so without this the run would judge the
fixtures, not the TP.

The optional second argument is a file the listener appends one JSON line to
for every request a TP's body sends (test, method, URL with its query,
header names, and the JSON body it carries, up to 64 KiB), and for every
request a fixture sends, marked `fixture` with the suite file it belongs to (a
suite's setup has no test): dev/etsi-view-diff.py reads it to tell a refusal
the view owes from a deviation, a hidden type or Attribute named in the body
(T-3787) or a write after a fixture's registration (T-3945) included.

A TP's teardown is written for the root, so a refusal or a local write the
view owes can leave an Entity behind for the next TP's setup to trip over
(409 against a redirect registration): a write through the view stays local
where the root would forward it (ADR-0023 rule 7, D018_03), and a purge the
view refuses leaves what the TP's setup created (D017_01_inc). So every
Entity a TP creates, in its setup or its body (`POST /entities`,
`entityOperations/create|upsert`), is deleted at the setup root with
`local=true` once the TP and its teardown are over (T-3954). A suite setup's
Entities are the suite's and stay.

    robot --listener "dev/etsi_setup_url.py;http://localhost:9090/ngsi-ld/v1;requests.jsonl" ...
"""

import json
import urllib.error
import urllib.parse
import urllib.request

from robot.libraries.BuiltIn import BuiltIn

NAMES = ("${url}", "${temporal_api_url}")
FIXTURE = ("SETUP", "TEARDOWN")
METHODS = ("GET", "POST", "PUT", "PATCH", "DELETE")
BODY = ("json", "data")
MAX_BODY = 64 * 1024
CREATES = ("/entities", "/entityOperations/create", "/entityOperations/upsert")
TENANT = "ngsild-tenant"


def created_ids(method, url, body):
    """The Entity ids a request creates: a POST to `/entities` carries one
    Entity, to `entityOperations/create|upsert` a list of them."""
    if method != "POST" or not urllib.parse.urlsplit(url).path.rstrip("/").endswith(CREATES):
        return []
    entities = body if isinstance(body, list) else [body]
    return [e["id"] for e in entities if isinstance(e, dict) and isinstance(e.get("id"), str)]


class etsi_setup_url:  # Robot takes the class named as its module
    ROBOT_LISTENER_API_VERSION = 3

    def __init__(self, setup_url, record=None):
        self.setup_url = setup_url.rstrip("/")
        self.record = record
        self.under_test = None
        self.depth = 0
        self.test = None
        self.suites = []
        self.created = []  # (id, tenant) the running TP created

    def _point(self, at_setup):
        builtin = BuiltIn()
        if self.under_test is None:
            self.under_test = {n: builtin.get_variable_value(n) for n in NAMES}
        for name in NAMES:
            builtin.set_global_variable(name, self.setup_url if at_setup else self.under_test[name])

    def start_suite(self, data, result):
        self.suites.append(str(data.source or "").split("ngsi-ld-test-suite/")[-1])

    def end_suite(self, data, result):
        self.suites.pop()

    def start_test(self, data, result):
        # the key dev/etsi-report.py's tp-status.csv gives the same TP
        source = str(getattr(data, "source", None) or data.parent.source or "")
        self.test = f"{source.split('ngsi-ld-test-suite/')[-1]}::{data.name}"

    def end_test(self, data, result):
        self.test = None
        created, self.created = self.created, []
        for entity_id, tenant in dict.fromkeys(created):
            self._delete(entity_id, tenant)

    def _delete(self, entity_id, tenant):
        """Delete one Entity at the setup root, local only; an Entity the view
        refused to create (404) or the teardown removed is fine."""
        url = f"{self.setup_url}/entities/{urllib.parse.quote(entity_id, safe='')}?local=true"
        req = urllib.request.Request(url, method="DELETE")
        if tenant:
            req.add_header("NGSILD-Tenant", tenant)
        try:
            urllib.request.urlopen(req, timeout=10).close()
        except urllib.error.HTTPError as e:
            if e.code != 404:
                print(f"etsi_setup_url: DELETE {url}: {e.code}")
        except OSError as e:
            print(f"etsi_setup_url: DELETE {url}: {e}")

    def start_keyword(self, data, result):
        if result.type in FIXTURE:
            if self.depth == 0:
                self._point(at_setup=True)
            self.depth += 1
        # A step after a failure still starts, NOT RUN, and a variable an earlier step would have
        # assigned does not exist: no request left, so nothing to record or delete (T-4030).
        elif data.name in METHODS and getattr(result, "owner", None) == "RequestsLibrary" \
                and not result.not_run and (self.depth > 0 or self.test):
            fixture = self.depth > 0
            line = self._request(data.name, data.args)
            tenant = line.pop("tenant", None)
            if self.test:
                self.created += [(i, tenant) for i in
                                 created_ids(data.name, line["url"], line.get("body"))]
            if self.record:
                if fixture:
                    line.update(fixture=True, suite=self.suites[-1] if self.suites else "")
                with open(self.record, "a", encoding="utf-8") as fh:
                    fh.write(json.dumps(line) + "\n")

    def end_keyword(self, data, result):
        if result.type in FIXTURE:
            self.depth -= 1
            if self.depth == 0:
                self._point(at_setup=False)

    def _request(self, method, args):
        """The request as RequestsLibrary sends it: `url=` or the first
        positional argument, `headers=` resolved in the caller's scope."""
        builtin = BuiltIn()
        named = {}
        positional = []
        for arg in args:
            key, sep, value = str(arg).partition("=")
            if sep and key in ("url", "headers", "params") + BODY:
                named[key] = value
            else:
                positional.append(str(arg))
        url = named.get("url") or (positional[0] if positional else "")
        url = str(builtin.replace_variables(url))
        headers = builtin.replace_variables(named["headers"]) if "headers" in named else {}
        params = builtin.replace_variables(named["params"]) if "params" in named else ""
        if isinstance(params, dict):
            params = "&".join(f"{k}={v}" for k, v in params.items())
        if params:
            url += ("&" if "?" in url else "?") + str(params)
        headers = headers if isinstance(headers, dict) else {}
        line = {"test": self.test, "method": method, "url": url,
                "headers": sorted(str(k).lower() for k in headers)}
        tenant = next((v for k, v in headers.items() if str(k).lower() == TENANT), None)
        if tenant:
            line["tenant"] = str(tenant)
        body = next((named[k] for k in BODY if k in named), None)
        if body is not None:
            line["body"] = self._json(builtin.replace_variables(body))
        return line

    @staticmethod
    def _json(value):
        """The body as JSON: a dict or list as Robot holds it, a string parsed,
        anything larger than MAX_BODY or not JSON left out (None)."""
        if isinstance(value, (bytes, bytearray)):
            value = value.decode("utf-8", "replace")
        if isinstance(value, str):
            if len(value) > MAX_BODY:
                return None
            try:
                return json.loads(value)
            except ValueError:
                return None
        if isinstance(value, (dict, list)):
            text = json.dumps(value, default=str)
            return value if len(text) <= MAX_BODY else None
        return None
