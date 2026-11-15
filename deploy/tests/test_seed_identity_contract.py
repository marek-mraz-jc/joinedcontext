"""dev's seed follows the identity contract (CC-82, PF-84, PL-57, EP-77; T-1449).

Every space that predates rendered segments pins today's segment, so no entity id changes;
a mapping reads its space from `env("JC_SPACE")`; Helsinki's policies name the organization as
`did:web:{orgDomain}`; a reference inside the organization names its Endpoint, not its slug.
"""

import re
from pathlib import Path

import yaml

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
# The seeds name the demo people by their fixed Keycloak ids (PF-126).
DEMO_IDS = {
    name: user["id"]
    for name, user in yaml.safe_load(
        (SEED.parent.parent / "keycloak/demo-users.yaml").read_text()
    ).items()
}


def documents(path: Path):
    return [doc for doc in yaml.safe_load_all(path.read_text()) if isinstance(doc, dict)]


def manifests(kind: str):
    for path in sorted(SEED.glob("*/*.yaml")):
        if path.name.endswith("-bento.yaml") or path.name == "index.yaml":
            continue
        for doc in documents(path):
            if doc.get("kind") == kind:
                yield path, doc


def pinned(space: dict) -> str:
    """The segment a seeded space pins: its name, except an organization's catalogue slice
    (T-5493), which pins `{project}-catalog`, the segment the Portal and the gateway read as a
    project's slice of the installation's catalogue."""
    meta = space["metadata"]
    if meta["name"] == "catalog" and meta["namespace"] != "slovensko":
        return f"{meta['namespace']}-catalog"
    return meta["name"]


def test_every_space_pins_the_segment_its_ids_carry_today():
    spaces = list(manifests("ContextSpace"))
    assert spaces, "the seed holds spaces"
    for path, space in spaces:
        assert space["spec"].get("urnSegment") == pinned(space), path.name


def test_no_mapping_types_its_space_or_project_in():
    # Each mapping against the spaces and the name of the projects its seed folder's manifests
    # live in: a Slovak events mapping that names the town "bratislava" is not typing the space
    # bratislava in (T-5271).
    names: dict[str, set] = {}
    for _, doc in manifests("Project"):
        names.setdefault(doc["metadata"]["name"], set()).add(doc["metadata"]["name"])
    for _, doc in manifests("ContextSpace"):
        names[doc["metadata"]["namespace"]].add(doc["spec"]["urnSegment"])
    folders: dict[str, set] = {}
    for path in sorted(SEED.glob("*/*.yaml")):
        if not path.name.endswith("-bento.yaml") and path.name != "index.yaml":
            for doc in documents(path):
                if doc.get("metadata", {}).get("namespace") in names:
                    folders.setdefault(path.parent.name, set()).update(names[doc["metadata"]["namespace"]])
    bentos = sorted(SEED.glob("*/*-bento.yaml"))
    assert bentos, "the seed holds mappings"
    assert {p.parent.name for p in bentos} <= set(folders), "every mapping's folder names its project"
    minted = 0
    for path in bentos:
        literal = re.compile(r'"(%s)"' % "|".join(map(re.escape, sorted(folders[path.parent.name]))))
        for number, line in enumerate(path.read_text().splitlines(), 1):
            stripped = line.lstrip()
            # `let source_url` is the publisher's own address, and one of them lives under
            # `egov.banskabystrica.sk`: a host that happens to contain a project's org domain
            # is not this mapping typing its project in (T-2305).
            if stripped.startswith("#") or stripped.startswith("let source_url ="):
                continue
            assert not literal.search(line), f"{path.name}:{number}: {stripped}"
        minted += 'env("JC_SPACE")' in path.read_text()
    # Every mapping mints its ids from the space in the environment. The two reapers that minted
    # none are gone: their entities carry `expiresAt` now (T-4823).
    silent = {p.name for p in bentos if 'env("JC_SPACE")' not in p.read_text()}
    assert silent == set(), silent
    assert minted == len(bentos)


def test_helsinki_policies_name_the_organization_by_placeholder():
    policies = [(p, d) for p, d in manifests("Policy") if p.parent.name == "helsinki"]
    # Twelve of the city's own, the service map App's and the data grids App's (T-2788), and the
    # mobility project's station App's, seeded in this folder with the rest of hel.fi (T-3185),
    # and the members' read on the operations endpoint (bikes-ops-read, T-3957), and one public
    # read per dataset split out of helsinki-all (T-4344), and the history of helsinki-all's types
    # that keep one (T-4372), and the pipelines' delete of withdrawn Alerts (T-4823), and the
    # steward's import of journey 41's test places (T-4867), and the steward's add and delete of
    # the data workspace journey's test alerts (T-5092), and the public read of the street cleaning
    # air pilot (T-5255), and one public read per area statistics theme (T-5258), and the public read
    # of air quality (T-5259), and the service usage counters (T-5258), and the public reads of the heat vulnerability index and the service
    # property energy use (T-5278), and the public reads of the cycle and traffic counts (T-5253), and of feedback reports (T-5257), and its catalogue's
    # Portal write and public read (T-5493), and the public reads of the autumn traffic volumes and
    # the machine vision counts (T-5669), and the own-account writes of the two pipelines that
    # write several types (T-5717), and the public read of the heritage maps (T-5670), and of the
    # traffic signal junctions (T-5286), and those of the noise survey and the tree register (T-5378), and of the nature information system (T-5823), and the steward's version read (T-6348),
    # and the steward's hand on the attribute history journey's station (T-5326).
    assert len(policies) == 53
    for path, policy in policies:
        assert policy["spec"]["assigner"] == "did:web:{orgDomain}", path.name


def test_a_reference_inside_the_organization_names_its_endpoint():
    endpoints = {
        (doc["metadata"]["namespace"], doc["metadata"]["name"]) for _, doc in manifests("Endpoint")
    }
    references = list(manifests("SharedSpaceReference"))
    assert references
    for path, reference in references:
        spec = reference["spec"]
        assert "endpointSlug" not in spec, path.name
        target = (spec["endpointRef"]["project"], spec["endpointRef"]["name"])
        assert target in endpoints, f"{path.name} names {target}, which the seed does not hold"


def _index_destinations():
    """Every seeded manifest with the repository path its `index.yaml` writes it to."""
    for index in sorted(SEED.glob("*/index.yaml")):
        for source, destination in (yaml.safe_load(index.read_text()) or {}).items():
            path = index.parent / source
            if not path.exists() or not source.endswith((".yaml", ".yml")):
                continue
            docs = documents(path)
            # A multi-document file has no single kind to place; none exists in the seed today.
            if len(docs) != 1 or "kind" not in docs[0]:
                continue
            yield path, docs[0], destination


def _shape(destination: str, name: str) -> str:
    """The destination with its variable segments named, so two manifests of one kind compare."""
    segments = destination.split("/")
    shaped = []
    for position, segment in enumerate(segments):
        previous = segments[position - 1] if position else None
        if previous in {"projects"}:
            shaped.append("{namespace}")
        elif previous in {"spaces"}:
            shaped.append("{space}")
        elif previous in {"pipelines", "blueprints", "apps"}:
            shaped.append("{name}")
        elif position == len(segments) - 1 and segment == f"{name}.yaml":
            shaped.append("{name}.yaml")
        else:
            shaped.append(segment)
    return "/".join(shaped)


# Kinds jc-core places at both levels (PATH_TEMPLATE and PROJECT_PATH_TEMPLATE): the org-wide
# open-data connector sits at mcp/{name}.yaml, each publisher's at projects/{project}/mcp/ (T-5367).
_ORG_OR_PROJECT = {"McpServer"}


def test_every_kind_is_seeded_to_one_repository_path():
    """MF-06: a manifest lives at the path its kind declares, or the loader refuses it.

    `jcctl validate --repo-dir` over the rendered seed reported
    `SharedSpaceReference/bbsk/mesto-kpi belongs at projects/bbsk/shared/mesto-kpi.yaml`
    because one `index.yaml` wrote `shares/` where the kind's template says `shared/` (T-1471).
    Comparing the seeded manifests of one kind against each other needs no jcctl in the lane.
    Its ceiling: a kind seeded exactly once has nothing to disagree with.
    """
    shapes: dict[str, dict[str, list[str]]] = {}
    for _path, doc, destination in _index_destinations():
        shape = _shape(destination, doc.get("metadata", {}).get("name"))
        if doc["kind"] in _ORG_OR_PROJECT:
            shape = shape.removeprefix("projects/{namespace}/")
        shapes.setdefault(doc["kind"], {}).setdefault(shape, []).append(destination)
    assert shapes, "the seed holds manifests"
    disagreeing = {kind: paths for kind, paths in shapes.items() if len(paths) > 1}
    assert not disagreeing, f"one kind, two repository paths: {disagreeing}"


def test_the_demo_editor_proposes_and_deletes_nothing():
    """CC-34, PF-50, PF-58: dev holds a person whose own change waits for somebody else (T-2231).

    The steward is bound to `steward` and to `org-admin`, so `delete` makes their own change the
    PF-58 administrator exception; the approver holds `approve` and cannot propose; the viewer
    holds `read` and gets the missing-role refusal instead. Without a fourth person the plain
    self-approval refusal cannot be played on dev at all, which is what
    `joinedcontext-portal/ui/e2e/live/roles-refusals.spec.ts` needed.

    The property that journey stands on, read off `ui/src/api/approval.ts:88,100-106`: the editor
    may approve the kind they propose — otherwise the page answers `needsRole` and never reaches
    the self-approval rule — and may delete nothing, which is what `administers` asks for. They
    may also propose a `RoleBinding`, so the grant refusal is about the width of the grant and
    not about their being unable to grant at all (PF-50).
    """
    editor = DEMO_IDS["demo.editor"]
    roles = {doc["metadata"]["name"]: doc for _path, doc in manifests("Role")}
    held = [
        doc["spec"]["role"]
        for _path, doc in manifests("RoleBinding")
        if any(subject.get("user") == editor for subject in doc["spec"]["subjects"])
    ]
    assert held, f"{editor} is bound to no role"
    rules = []
    for name in held:
        assert name in roles, f"{editor} is bound to `{name}`, which the seed does not declare"
        rules.extend(roles[name]["spec"]["rules"])

    def verbs_of(kind: str) -> set[str]:
        return {verb for rule in rules if kind in rule["kinds"] for verb in rule["verbs"]}

    assert "delete" not in {verb for rule in rules for verb in rule["verbs"]}, (
        f"{editor} administers a kind, so their own change would be the PF-58 exception"
    )
    assert {"propose", "approve"} <= verbs_of("ContextSpace"), sorted(verbs_of("ContextSpace"))
    assert verbs_of("RoleBinding") == {"propose"}, sorted(verbs_of("RoleBinding"))

    # And strictly below the steward, who holds `org-admin` beside `steward`.
    steward_kinds = {
        kind
        for name in ("steward", "org-admin")
        for rule in roles[name]["spec"]["rules"]
        for kind in rule["kinds"]
    }
    assert {kind for rule in rules for kind in rule["kinds"]} <= steward_kinds


def test_the_organization_administrator_writes_groups():
    """PF-56, PF-62: the organization administrator acts on every kind, groups included (T-2625).

    A binding may name a group, so whoever writes bindings writes the groups they name. Without
    `Group` in `org-admin`, Organization → Groups and the Project settings journey refused the
    administrator with "no role grants propose on Group in project org (PF-50)".
    """
    roles = {doc["metadata"]["name"]: doc for _path, doc in manifests("Role")}
    granted = {
        verb
        for rule in roles["org-admin"]["spec"]["rules"]
        if "Group" in rule["kinds"]
        for verb in rule["verbs"]
    }
    assert {"propose", "approve", "delete"} <= granted, sorted(granted)


def test_the_janitor_reaches_journey_residue_and_nothing_seeded():
    """T-2627, PF-49: the residue sweep's approver deletes what a journey or a take named, alone.

    The owner's decision of 2026-09-24: a fifth demo person used only by the sweep, confined to
    journey-named resources, so the demo roles stay as they are and the Portal's rule (approve and
    delete for a removal) stays too. The pattern is read as the Portal and roles.rego read it, the
    whole name, and no name the seed or a sample app commits may fall inside it: a janitor that
    could approve the removal of `helsinki` would be an administrator by another name.
    """
    janitor = DEMO_IDS["demo.janitor"]
    roles = {doc["metadata"]["name"]: doc for _path, doc in manifests("Role")}
    held = [
        doc
        for _path, doc in manifests("RoleBinding")
        if any(subject.get("user") == janitor for subject in doc["spec"]["subjects"])
    ]
    # Beside its own role the janitor reads `hel` as `viewers` names it: the read it held through
    # `platform-readers` before PF-61 ended (T-6415), no more.
    by_role = {doc["spec"]["role"]: doc for doc in held}
    assert sorted(by_role) == ["janitor", "viewer"], "the janitor holds its own role and the old read alone"
    assert by_role["viewer"]["metadata"]["name"] == "viewers"
    assert by_role["janitor"]["spec"]["scope"] == {"project": "helsinki"}
    patterns = []
    for rule in roles["janitor"]["spec"]["rules"]:
        assert set(rule["verbs"]) == {"approve", "delete"}, "the janitor proposes and reads nothing"
        (constraint,) = rule["constraints"]
        assert constraint["field"] == "metadata.name" and set(constraint) == {"field", "pattern"}
        patterns.append(re.compile(constraint["pattern"]))

    def reached(name: str) -> bool:
        return any(pattern.fullmatch(name) for pattern in patterns)

    # What the sweep found on dev on 2026-09-25 (ui/e2e/live/residue.spec.ts names the same).
    for name in ["t1588-mucg4n37", "t1589r-bikes", "t1550-1452", "bike-network-db-1101",
                 "hsl-citybikes-draft-0837", "citybikes-0915", "bikes-regional-ab12", "large-map-city",
                 # What T-4993 found on dev on 2026-10-21: journeys of four-digit tasks above T-1999.
                 "t4843-ht6nrt", "t4668-model-0636", "t4869-files-mvhnojg5", "newuser-policy-mvfkrjlc"]:
        assert reached(name), f"{name} is residue the janitor cannot approve away"
    # `city-bikes` a take once created is the seeded dashboard now.
    for name in ["helsinki", "city-bikes", "helsinki-t1588-x", "citybikes-09150", "t1588", "", "t1588-X",
                 "t48431-x", "newuser-policy-", "newuser-policy-A"]:
        assert not reached(name), f"{name} is not a journey's"

    # The binding's scope (asserted above) bounds what it reaches: the seed of that project, and the
    # sample apps. A year-suffixed table of another project (`bratislava-rozpocet-prijmy-2019`, T-5676)
    # stays outside it.
    apps = SEED.parent.parent / "gitea/apps"
    project = by_role["janitor"]["spec"]["scope"]["project"]
    seeded = {
        doc["metadata"]["name"]
        for path in [*SEED.glob(f"{project}/*.yaml"), *apps.glob("*/app.yaml"), *apps.glob("*/grants/**/*.yaml")]
        if path.name != "index.yaml" and not path.name.endswith("-bento.yaml")
        for doc in documents(path)
        if isinstance(doc.get("metadata"), dict) and "name" in doc["metadata"]
    }
    assert len(seeded) > 50, "the seed and the sample apps were read"
    assert not sorted(name for name in seeded if reached(name)), "the janitor reaches a seeded resource"


def test_every_bound_group_is_seeded_or_the_identity_providers():
    """PF-63, PF-64: a binding's group is a seeded `Group` or marked `source: provider` (T-2844).

    `platform-readers` is the realm's default group: Keycloak owns its members and no `Group`
    manifest declares it, so without the mark `jcctl validate` on dev's repository reports the
    binding as naming a group that does not exist. A marked group seeded as a `Group` as well
    would give the reconciler a group the provider owns.
    """
    seeded = {doc["metadata"]["name"] for _path, doc in manifests("Group")}
    bound = [
        (path.name, subject)
        for path, doc in manifests("RoleBinding")
        for subject in doc["spec"]["subjects"]
        if "group" in subject
    ]
    assert bound, "the seed binds groups"
    for name, subject in bound:
        if subject.get("source") == "provider":
            assert subject["group"] not in seeded, f"{name}: {subject['group']} is the provider's and seeded"
        else:
            assert "source" not in subject, f"{name}: unknown source {subject['source']}"
            assert subject["group"] in seeded, f"{name}: {subject['group']} is no seeded Group and not marked provider"


def test_every_quality_rule_name_is_unique_in_its_space():
    # The Portal refuses a whole space whose quality rules share a name (DM-87); dev kept
    # barcelona's last valid space while six rules were all `in-area` (T-5265, T-5264).
    for path, space in manifests("ContextSpace"):
        names = [rule["name"] for rule in space["spec"].get("quality", [])]
        assert len(names) == len(set(names)), f"{path.name}: {sorted(n for n in names if names.count(n) > 1)}"


def _duration_ok(text) -> bool:
    """A heartbeat the Portal admits (DM-109): an ISO 8601 duration of PT1M to P3650D."""
    match = re.fullmatch(r"P(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?", str(text))
    if not match or text in ("P", "PT"):
        return False
    d, h, m, s = (int(g or 0) for g in match.groups())
    return 60 <= d * 86400 + h * 3600 + m * 60 + s <= 3650 * 86400


def test_every_on_change_recording_has_a_heartbeat():
    # The Portal refuses a whole space whose retention records onChange without a heartbeat
    # (DM-109): prod kept helsinki's last valid space, one without the Alert net `current: PT1H`,
    # so 41 Fintraffic Alerts written before their lifetime never left (T-5792).
    for path, space in manifests("ContextSpace"):
        for rule in space["spec"].get("retention", []):
            recordings = {"": rule, **{f".{name}": own for name, own in rule.get("attributes", {}).items()}}
            for where, own in recordings.items():
                if own.get("record") == "onChange":
                    assert _duration_ok(own.get("heartbeat")), f"{path.name}: {rule['entityType']}{where}"


def test_the_heartbeat_check_reads_durations_as_the_portal_does():
    assert all(map(_duration_ok, ("PT1M", "PT1H", "P7D", "P30D", "P3650D")))
    assert not any(map(_duration_ok, (None, "PT30S", "P3651D", "P", "PT", "1h")))


# --- ADR-N-067 2.1 (T-6415): PF-61 ends, viewers are named ------------------------------------

PEOPLE = yaml.safe_load((SEED.parent.parent / "keycloak/demo-users.yaml").read_text())
# The demo people `platform-readers` made viewer of `hel` before T-6415: every account in the
# default group, less the two members T-6408 added (they carry an Organization subgroup) and the
# people of another Organization's root group, whom a Helsinki binding may not name (PF-123).
OTHER_ORGANIZATIONS = {"zilina", "barcelona", "bratislava"}
PF61_READERS = {
    "demo.steward", "demo.viewer", "demo.approver", "demo.editor", "demo.janitor", "demo.probe",
}


def subject_groups():
    for path, doc in manifests("RoleBinding"):
        for subject in doc["spec"]["subjects"]:
            if "group" in subject:
                yield path.name, subject["group"]


def test_no_binding_makes_the_default_group_a_project_person():
    """Every account lands in `platform-readers`, so a binding of it made every realm user a
    viewer of every project of `hel` (PF-61). A member reaches no project until someone binds a
    role (ADR-N-067 2.1)."""
    bound = [(name, group) for name, group in subject_groups() if group == "platform-readers"]
    assert not bound, bound


def test_the_people_who_read_hel_through_pf61_are_named_viewers():
    """Each Helsinki person who read `hel` through `platform-readers` is a subject of `viewers` by
    their Keycloak id, and nobody else is (no project added). Another Organization's people lose
    `hel`: PF-123 keeps a binding inside its Organization."""
    readers = {
        name for name, user in PEOPLE.items()
        if str(user.get("platformReaders")) != "False"
        and not any("/" in group for group in user["groups"])
        and not OTHER_ORGANIZATIONS & set(user["groups"])
    }
    assert readers == PF61_READERS, sorted(readers ^ PF61_READERS)
    [viewers] = [doc["spec"] for _path, doc in manifests("RoleBinding") if doc["metadata"]["name"] == "viewers"]
    assert viewers["role"] == "viewer" and viewers["scope"] == {"organization": "hel"}
    named = {subject["user"] for subject in viewers["subjects"]}
    assert named == {DEMO_IDS[name] for name in PF61_READERS}, sorted(named)


def assignees(spec: dict):
    if "assignee" in spec:
        yield spec["assignee"]
    for rule in spec.get("rules") or []:
        if "assignee" in rule:
            yield rule["assignee"]


def test_no_holder_of_a_root_group_grant_carries_jc_groups():
    """With `jc_groups` in the token the PDP matches a `group` grant only as `/{org}/{group}`
    (T-6409), so a grant naming a root group stops reaching a person the moment they join an
    Organization subgroup. Since T-6514 every bound person does, so bikes-ops-read and
    kpi-shared-read name the derived `{project}-readers`; a plain project group left in a grant
    answers its members 403 on dev."""
    apps = SEED.parent.parent / "gitea/apps"
    policies = [doc for _path, doc in manifests("Policy")] + [
        doc for path in sorted(apps.rglob("*.yaml")) for doc in documents(path) if doc.get("kind") == "Policy"
    ]
    root = {
        a["id"] for doc in policies for a in assignees(doc["spec"])
        if a.get("kind") == "group" and "/" not in a.get("id", "")
    }
    assert {"helsinki-readers", "bbsk-readers"} <= root, sorted(root)
    assert not {"helsinki", "bbsk"} & root, sorted(root)
    for name, user in PEOPLE.items():
        if any("/" in group for group in user["groups"]):
            held = root & set(user["groups"])
            assert not held, f"{name} carries jc_groups and loses the grants of {sorted(held)}"
