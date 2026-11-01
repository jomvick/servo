#!/bin/bash
# Phase 0 : construit la sonde, puis lance chaque test dans SON PROCESSUS.
#
# Un processus par test n'est pas un confort : `Opts` est un singleton de processus
# (`components/config/opts.rs:279`, `OnceLock::set` -> panique « Already initialized »),
# donc deux `Servo::new()` dans le meme processus sont impossibles. C'est aussi une
# contrainte d'architecture pour l'embed : un moteur par processus, onglets = WebViews.
set -o pipefail
cd "$(dirname "$0")/../.." || exit 1
export CARGO_BUILD_JOBS=2
export CARGO_PROFILE_DEV_DEBUG=0

echo "=== build des tests ==="
cargo test -p servo --test phase0_probe --no-run 2>&1
status=$?
echo "=== build status : $status ==="
[ $status -ne 0 ] && exit $status

BIN=$(ls -t target/debug/deps/phase0_probe-* 2>/dev/null | grep -v '\.d$' | head -1)
echo "=== binaire de test : $BIN ==="

for test in probe_frame_path_and_cost probe_userscript_subframe_reach; do
    echo "=== ${test} ==="
    /usr/bin/time -v "$BIN" "$test" --exact --nocapture 2>&1 |
        grep -E "PROBE-|^test |Maximum resident set size|Elapsed \(wall|panicked|assertion"
done
