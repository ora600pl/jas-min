#!/bin/sh
# Portable plugin commands must stay inside the package; resolve assets from here,
# independently of the user's project (for example DNV).
set -eu
cd -- "$(dirname -- "$0")"
exec node ./server.mjs
