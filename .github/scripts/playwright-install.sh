#!/bin/sh
# Installs Chromium and its system packages for the pip-installed playwright, in three tries:
# the runner's apt mirrors (security.ubuntu.com, azure.archive.ubuntu.com) failed DNS in three
# runs in a row (43031791849, 43037195781, 43044556087), each with no verdict. Fails when all do.
# Each part of a try is bounded (TRY_SECONDS, default 300): a hung apt or download held smoke
# 44665071709 45 min (T-7442). The apt part runs as `sudo timeout`, so the timeout runs as root
# and its group signal reaches apt-get and dpkg: a plain `timeout ... --with-deps` killed only
# the unprivileged playwright, its sudo apt-get lived on and held the dpkg lock, and tries 2 and
# 3 failed at once on "Could not get lock /var/lib/dpkg/lock-frontend" (smoke 44756758897, T-7456).
# Before a retry the script waits (bounded) for any apt-get or dpkg left over, and apt itself
# waits up to TRY_SECONDS for the lock (DPkg::Lock::Timeout) instead of failing. A try killed
# mid-unpack leaves "dpkg was interrupted" and every later apt-get refuses, so each try first
# finishes what dpkg left (`dpkg --configure -a`, a no-op on a clean runner).
#   sh playwright-install.sh
set -u
TRY_SECONDS=${TRY_SECONDS:-300}
PY=$(command -v python)

# apt_idle: waits until no apt-get or dpkg runs, at most TRY_SECONDS
apt_idle() {
  waited=0
  while pgrep -x 'apt-get|apt|dpkg' >/dev/null; do
    [ "$waited" -ge "$TRY_SECONDS" ] && { echo "apt still busy after ${TRY_SECONDS}s"; return 1; }
    sleep 5
    waited=$((waited + 5))
  done
}

for i in 1 2 3; do
  apt_idle &&
    echo "DPkg::Lock::Timeout \"$TRY_SECONDS\";" | sudo tee /etc/apt/apt.conf.d/99jc-lock-timeout >/dev/null &&
    sudo timeout -k 30 "$TRY_SECONDS" env DEBIAN_FRONTEND=noninteractive dpkg --configure -a &&
    sudo timeout -k 30 "$TRY_SECONDS" env DEBIAN_FRONTEND=noninteractive "$PY" -m playwright install-deps chromium &&
    timeout -k 30 "$TRY_SECONDS" "$PY" -m playwright install chromium &&
    exit 0
  echo "playwright install try $i failed"
  [ "$i" = 3 ] || sleep 30
done
exit 1
