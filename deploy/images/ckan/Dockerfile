# The catalogue image: stock CKAN plus the one extension a Keycloak login needs (T-0403, OPS-26).
#
# `ckan/ckan-base` carries no OIDC authenticator, and CKAN exits on a plugin it cannot import,
# so `global.ckan.sso` only works against this image. The extension is `ckanext-oidc-pkce` and
# not the `ckanext-oauth2` the task named: that one still calls `before_map()` and reads
# `repoze.who.identity`, both of which CKAN dropped in 2.10, and its last commit is from 2019.
# `ckanext-oidc-pkce` is tested against 2.9, 2.10 and 2.11 upstream and speaks plain OIDC, which
# is what the realm serves.
#
# Nothing else is added: the theme is still mounted from a ConfigMap (OPS-46), so a rebrand stays
# a values edit and never a rebuild.
FROM ckan/ckan-base:2.11.7@sha256:822c79ad696a8f7539c8762f1a0455d6957ae69a451f49bfa2eec2a0ec6d0f1d AS ckan

USER root

# Pinned like every other dependency here (OPS-27): a floating version is a different login
# flow on the next build. `--no-cache-dir` keeps the layer to what is installed.
RUN pip install --no-cache-dir 'ckanext-oidc-pkce==0.4.1'

# DCAT-AP for harvesters (T-3010): /catalog.{rdf,ttl,jsonld} and /dataset/{id}.{rdf,ttl,jsonld}
# with the `euro_dcat_ap_3` profile. 2.4 is the line that supports CKAN 2.11 and DCAT-AP 3; the
# theme's `jc_dcat_ap` profile (charts/ckan/files/jc_theme/dcat_profile.py) runs on top of it.
RUN pip install --no-cache-dir 'ckanext-dcat==2.4.4'

# The Python packages CKAN pins that have a fix published. CKAN's own pins are floors, not
# ceilings, and each of these is a patch release of a library CKAN uses through a stable API.
RUN pip install --no-cache-dir --upgrade \
        'urllib3>=2.7.0' \
        'PyJWT>=2.13.0' \
        'Mako>=1.3.12' \
        'wheel>=0.46.2' \
        'jaraco.context>=6.1.0' \
        'setuptools>=78.1.1' \
        'msgpack>=1.2.1' \
        'certifi>=2026.10.19'

# The table view showed only `_id`, `entity_id` and `type` (T-3047): every other DataStore column
# of a published endpoint is named after its NGSI-LD path (`dataProvider.value`), and CKAN 2.11's
# datatables_view hands DataTables that name as the column's `data`, which DataTables reads as
# nested-object notation (`row.dataProvider.value`) and finds empty. A function accessor reads the
# key as it is. Search and sort are unaffected: the ajax view reads each column's `name`, never its
# `data`. The grep before and after fails the build when an upgrade moves the line, so a new base
# image never ships the empty columns silently.
RUN f=/srv/app/src/ckan/ckanext/datatablesview/assets/datatablesview.js \
    && grep -q "const colDict = { name: colDefn.id, data: colDefn.id, contentPadding: 'MM' }" "$f" \
    && sed -i "s/const colDict = { name: colDefn.id, data: colDefn.id, contentPadding: 'MM' }/const colDict = { name: colDefn.id, data: (row) => row[colDefn.id], contentPadding: 'MM' }/" "$f" \
    && grep -q "data: (row) => row\[colDefn.id\], contentPadding" "$f" \
    && ! grep -q "data: colDefn.id" "$f"


# Everything this stage installed, at the versions it resolved, so the stages below rebuild the
# same set and nothing else.
RUN pip freeze --exclude-editable > /tmp/requirements.txt

# ckan/ckan-base sits on Debian 12, whose sqlite, zlib and kernel headers carry CRITICAL
# findings with no fix on that release, and 2.11.7 is the newest base there is (T-4128). The
# image therefore keeps only CKAN from it: the Python packages above are rebuilt on Debian 13,
# the C extensions (psycopg2, uWSGI) against its libraries, and the runtime stage carries no
# compiler, no headers and no git.
FROM python:3.10-slim-trixie@sha256:6ff506466f8b1e981719b468cc0023b8ef000b983cdec0eafe119d01f379e31d AS build

RUN apt-get update \
    && apt-get install -y --no-install-recommends gcc g++ libpq-dev \
    && rm -rf /var/lib/apt/lists/*

COPY --from=ckan /tmp/requirements.txt /tmp/requirements.txt
COPY --from=ckan /srv/app /srv/app
# setuptools 81 as the base pins it (CKAN still imports pkg_resources); markdown from source as
# the base builds it.
RUN pip install --no-cache-dir 'setuptools==81' 'wheel>=0.46.2' \
    && pip install --no-cache-dir --no-binary markdown -r /tmp/requirements.txt \
    && pip install --no-cache-dir --no-deps -e /srv/app/src/ckan -e /srv/app/src/ckanext-envvars \
    && python3 -m pip uninstall --yes pip \
    && rm -rf /usr/local/lib/python3.10/site-packages/pip-*.dist-info /usr/local/bin/pip*

# The dataset map (T-4639): MapLibre GL JS 6.14.0 (BSD-3-Clause, published 2026-10-11), the
# npm release checked against its sha256, served by CKAN from /vendor beside the theme so no page
# reaches a CDN. Its ES modules (1.7 MB) outgrow the theme's ConfigMap, hence the image. The
# worker module is found next to the main one (import.meta.url), so the two stay side by side.
ADD --checksum=sha256:f799682e7a2d189e43d18ccf6889d09d1e8547e2cab4ba79d7360cc96630207e \
    https://registry.npmjs.org/maplibre-gl/-/maplibre-gl-6.14.0.tgz /tmp/maplibre-gl.tgz
RUN install -d /srv/app/vendor/maplibre-gl-6.14.0 \
    && tar -xzf /tmp/maplibre-gl.tgz -C /tmp package/dist/maplibre-gl.mjs package/dist/maplibre-gl-worker.mjs \
        package/dist/maplibre-gl.css package/LICENSE.txt \
    && cp /tmp/package/dist/maplibre-gl.mjs /tmp/package/dist/maplibre-gl-worker.mjs /tmp/package/dist/maplibre-gl.css \
        /tmp/package/LICENSE.txt /srv/app/vendor/maplibre-gl-6.14.0/ \
    && rm -rf /tmp/maplibre-gl.tgz /tmp/package

FROM python:3.10-slim-trixie@sha256:6ff506466f8b1e981719b468cc0023b8ef000b983cdec0eafe119d01f379e31d

# The security updates that exist on the day the image is built (OPS-27); libpq and libmagic are
# what psycopg2 and python-magic load, locales the en_US.UTF-8 the base runs under.
RUN apt-get update \
    && apt-get upgrade -y --no-install-recommends \
    && apt-get install -y --no-install-recommends libpq5 libmagic1t64 locales \
    && sed -i '/en_US.UTF-8/s/^# //' /etc/locale.gen \
    && locale-gen \
    && echo UTC > /etc/timezone \
    && rm -rf /var/lib/apt/lists/*

# The users, paths and environment of ckan/ckan-base, so start_ckan.sh, the chart's entrypoint
# scripts and its volumes see the image they were written for.
RUN groupadd -g 502 ckan-sys \
    && useradd -rm -d /srv/app -s /bin/bash -g ckan-sys -u 502 ckan-sys \
    && useradd -rm -d /srv/app -s /bin/bash -g ckan-sys -u 503 ckan

# The runtime base's own pip and setuptools would survive a COPY on top of them, with the old
# vendored wheel and jaraco.context a scan reports: the build stage's /usr/local replaces them.
RUN rm -rf /usr/local/lib/python3.10/site-packages /usr/local/bin/pip*
COPY --from=build /usr/local /usr/local
COPY --from=build --chown=ckan-sys:ckan-sys /srv/app /srv/app

RUN chmod 755 /srv/app \
    && chown ckan:ckan-sys /srv/app/ckan.ini \
    && chown -R ckan:ckan-sys /srv/app/src \
    && install -d -m 755 -o ckan-sys -g ckan-sys /docker-entrypoint.d \
    && install -d -o ckan -g ckan-sys /var/lib/ckan

ENV TZ=UTC \
    APP_DIR=/srv/app \
    SRC_DIR=/srv/app/src \
    CKAN_INI=/srv/app/ckan.ini \
    PIP_SRC=/srv/app/src \
    CKAN_STORAGE_PATH=/var/lib/ckan \
    CKAN_SITE_URL=http://localhost:5000 \
    CKAN__PLUGINS="image_view text_view datatables_view datastore envvars" \
    UWSGI_HARAKIRI=50 \
    LC_ALL=en_US.UTF-8 \
    CKAN_REF=ckan-2.11.7

WORKDIR /srv/app
USER ckan
EXPOSE 5000
CMD ["/srv/app/start_ckan.sh"]
