#!/bin/sh
# Headless check of this build on the device, over SSH or from a terminal
# app: no display, no audio, no SDL call, no network. It runs the
# conformance oracles with each binary of the package and compares them
# with what the desktop build printed for the same carts (probe.expected,
# written when the package was made), then prints what the kernel says
# about memory. Everything goes to probe.log too.
#
#   cd /mnt/SDCARD/Emu/KUULA && sh probe.sh
#
# Exit status 0 when every check passed.

mydir=$(cd "$(dirname "$0")" && pwd)
cd "$mydir" || exit 1
export HOME="$mydir"
export XDG_DATA_HOME="$mydir/probe/data"
export LD_LIBRARY_PATH="$mydir/libs:/config/lib:/customer/lib:/usr/miyoo/lib:$LD_LIBRARY_PATH"
chmod +x ./kuula ./kuula-net 2>/dev/null

out="$mydir/probe"
carts="$mydir/probe-carts"
rm -rf "$out"
mkdir -p "$out"
fail=0

# expected KEY: the value after "KEY=" in probe.expected.
expected() {
    grep "^$1=" probe.expected | cut -d= -f2-
}

# check NAME GOT WANT: one line of verdict.
check() {
    if [ "$2" = "$3" ]; then
        echo "PASS $1"
    else
        echo "FAIL $1"
        echo "  got:  $2"
        echo "  want: $3"
        fail=1
    fi
}

{
    echo "== memory before"
    grep -e MemTotal -e MemFree -e MemAvailable /proc/meminfo

    # The binary without networking, then the one with it.
    for bin in kuula kuula-net; do
        [ -f "./$bin" ] || continue
        echo "== $bin"
        "./$bin" --version

        echo "== $bin: conformance, 60 frames: per-frame hashes"
        "./$bin" run "$carts/conformance" --headless --frames 60 --out "$out/$bin-conformance" > "$out/$bin-conformance.txt" 2>&1
        if cmp -s "$out/$bin-conformance/hashes.txt" "$carts/conformance/hashes.txt"; then
            echo "PASS $bin conformance hashes"
        else
            echo "FAIL $bin conformance hashes"
            diff "$out/$bin-conformance/hashes.txt" "$carts/conformance/hashes.txt" | head -5
            fail=1
        fi

        echo "== $bin: hello, 120 frames"
        "./$bin" run "$carts/hello" --headless --frames 120 > "$out/$bin-hello.txt" 2>&1
        check "$bin hello summary" "$(tail -n 1 "$out/$bin-hello.txt")" "$(expected hello_summary)"

        echo "== $bin: numeric, 502 frames: 120000 lines of libm and formatting"
        "./$bin" run "$carts/numeric" --headless --frames 502 > "$out/$bin-numeric.txt" 2>&1
        check "$bin numeric summary" "$(tail -n 1 "$out/$bin-numeric.txt")" "$(expected numeric_summary)"
        check "$bin numeric log" "$(sed '$d' "$out/$bin-numeric.txt" | md5sum | cut -d' ' -f1)" "$(expected numeric_log_md5)"
    done

    echo "== memory after"
    grep -e MemTotal -e MemFree -e MemAvailable /proc/meminfo
    if [ "$fail" = 0 ]; then echo "== ALL PASS"; else echo "== FAILED"; fi
} 2>&1 | tee "$mydir/probe.log"

grep -q "== ALL PASS" "$mydir/probe.log"
