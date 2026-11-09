#!/usr/bin/env python3
"""A rehearsal test person holding demo.editor's groups that the rehearsal realm has; writes '<user> <password>' to a 0600 file.

  new_person.py --kit <deploy/rehearsal dir> --base http://localhost:<port> --state <state> --groups g1,g2 --out <file>
"""
import argparse, os, sys
from pathlib import Path

ap = argparse.ArgumentParser()
for k in ("--kit", "--base", "--state", "--groups", "--out"):
    ap.add_argument(k, required=True)
ap.add_argument("--username", default="rehearsal-parity")
a = ap.parse_args()
sys.path.insert(0, a.kit)
import rehearsal as rh  # noqa: E402

rh.loopback_only(a.base)
h = rh.Http(a.base)
tok = rh.admin_token(h, Path(a.state))
_, tree = h.json("GET", "/id/admin/realms/jc/groups?briefRepresentation=false", headers={"Authorization": f"Bearer {tok}"})
have = {g.get("path") for g in tree or []}
want = [f"/{g}" for g in a.groups.split(",")]
groups = [g for g in want if g in have]
print("groups held:", groups, "absent in realm jc:", [g for g in want if g not in have])
_, password = rh.make_person(h, tok, "jc", a.username, groups=groups)
fd = os.open(a.out, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
os.write(fd, f"{a.username} {password}\n".encode())
os.close(fd)
