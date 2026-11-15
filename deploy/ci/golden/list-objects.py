#!/usr/bin/env python3
"""Print `apiVersion/Kind namespace/name` for every object in a rendered manifest,
sorted. The committed ci/golden/<environment>.txt is this output for each environment
ci.yml renders (T-4956), so a review sees exactly which objects a change adds or drops.
Helm test hooks are left out: `helmfile sync` never applies them.
The gitea seed ConfigMaps print one line per seed release (`gitea-seed-s<n>-*`, T-5450):
values/seed-layout.tpl packs files into parts by size, so any seed edit renames parts, and
part capacity is test_sample_apps_seed's and the seed-layout tests' to guard."""

import re
import sys

import yaml

SEED_PART = re.compile(r"^(gitea-seed-s\d+-).+$")


def is_test_hook(doc):
    hooks = ((doc.get("metadata") or {}).get("annotations") or {}).get("helm.sh/hook", "")
    return "test" in str(hooks).split(",")


def lines(docs):
    out, seeds = [], set()
    for d in docs:
        if not isinstance(d, dict) or is_test_hook(d):
            continue
        meta = d.get("metadata") or {}
        name = str(meta.get("name"))
        seed = d.get("apiVersion") == "v1" and d.get("kind") == "ConfigMap" and SEED_PART.match(name)
        line = "{}/{} {}/{}".format(
            d.get("apiVersion"),
            d.get("kind"),
            meta.get("namespace", "-"),
            SEED_PART.sub(r"\1*", name) if seed else name,
        )
        (seeds.add if seed else out.append)(line)
    return sorted(out + list(seeds))


if __name__ == "__main__":
    with open(sys.argv[1]) as fh:
        for line in lines(yaml.safe_load_all(fh)):
            print(line)
