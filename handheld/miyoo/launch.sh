#!/bin/sh
# Kuula on the Miyoo Mini (Plus) under OnionOS.
#
# Onion's standalone port launcher starts this from
# Roms/PORTS/Games/Kuula with HOME set to that directory and ./libs on
# LD_LIBRARY_PATH. With no argument the shell boots and lists ./carts;
# with a cart directory (for example carts/hello) that cart runs alone.
# Hold Menu for a second and a half to leave.

mydir=$(cd "$(dirname "$0")" && pwd)
cd "$mydir" || exit 1

export HOME="$mydir"
# Saves and settings: the SD card is the only writable place.
export XDG_DATA_HOME="$mydir/data"
# libSDL2 links the SigmaStar libraries the firmware keeps in /config/lib
# and /customer/lib.
export LD_LIBRARY_PATH="$mydir/libs:/config/lib:/customer/lib:/usr/miyoo/lib:$LD_LIBRARY_PATH"
mkdir -p "$XDG_DATA_HOME"
chmod +x ./kuula 2>/dev/null

# MainUI keeps its memory but stops using the CPU while a port runs.
main_ui=$(pidof MainUI)
[ -n "$main_ui" ] && kill -STOP $main_ui

if [ -n "$1" ]; then
    ./kuula run "$1" --in-process --scale 1 > kuula.log 2>&1
else
    ./kuula shell --carts ./carts --in-process --scale 1 > kuula.log 2>&1
fi
status=$?
echo "kuula exited with status $status" >> kuula.log

[ -n "$main_ui" ] && kill -CONT $main_ui
exit $status
