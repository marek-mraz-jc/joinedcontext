"""Runs inside the catalogue image: one dataset, or the catalogue page with it, as Turtle.

`python3 serialize.py dataset|catalog "<profiles>" < dataset.json`. CKAN's config is set the
way the chart sets it (charts/ckan/templates/ckan.yaml); the one stand-in is the catalogue's
"last modified" date, which CKAN reads from Solr and which the shapes do not constrain.
"""

import json
import os
import site
import sys

# ckanext-dcat and its libraries at the version images/ckan/Dockerfile pins, for an image
# built before it carried them. A site directory, because `ckanext` is a namespace package and
# only a site directory reads the `.pth` that extends it.
if os.environ.get("JC_DCAT_LIBS"):
    site.addsitedir(os.environ["JC_DCAT_LIBS"])

from ckan.common import config  # noqa: E402

mode, profiles = sys.argv[1], sys.argv[2]
config.update({
    "ckan.site_url": "https://data.dev.example",
    "ckan.site_title": "joinedcontext",
    "ckan.locale_default": "en",
    "ckanext.dcat.rdf.profiles": profiles,
})

from ckanext.dcat.processors import RDFSerializer  # noqa: E402  (after the config)
from ckanext.dcat.profiles import RDFProfile  # noqa: E402

dataset = json.load(sys.stdin)
RDFProfile._last_catalog_modification = lambda self: dataset["metadata_modified"]
serializer = RDFSerializer(profiles=profiles.split())
if mode == "dataset":
    print(serializer.serialize_dataset(dataset, _format="turtle"))
else:
    print(serializer.serialize_catalog({}, dataset_dicts=[dataset], _format="turtle"))
