#!/bin/sh
# Kuula on the Miyoo Mini (Plus) under OnionOS: Emu/KUULA/launch.sh.
#
# Onion's Games list calls this with a cart,
#   launch.sh /mnt/SDCARD/Roms/KUULA/<title>.cart
# and the two apps call it with nothing (the shell and its list of carts)
# or with --dev (the shell as a development receiver). Onion's own menu
# has exited by then and comes back when this returns.
#
# In a cart, Menu opens the pause menu, and its last entry ends the cart.
# Holding Menu for a second and a half leaves from anywhere.

progdir=$(cd "$(dirname "$0")" && pwd)
cd "$progdir" || exit 1

export HOME="$progdir"
# Saves and settings go under Onion's profile, where its backups and its
# guest mode cover them. Kuula adds kuula/ below this.
export XDG_DATA_HOME=/mnt/SDCARD/Saves/CurrentProfile/saves/Kuula
# libSDL2 links the SigmaStar libraries the firmware keeps in /config/lib
# and /customer/lib; Onion's path ends with another libSDL2, so ours is
# first.
export LD_LIBRARY_PATH="$progdir/libs:/config/lib:/customer/lib:/usr/miyoo/lib:$LD_LIBRARY_PATH"
mkdir -p "$XDG_DATA_HOME"

# A Mini Plus (model 354) has Wi-Fi and gets the binary with networking.
# Anything else gets the one without, and so does a Plus once a file named
# `offline` is put beside this script.
bin=./kuula
if [ "$(cat /tmp/deviceModel 2>/dev/null)" = 354 ] && [ -f ./kuula-net ] && [ ! -e ./offline ]; then
    bin=./kuula-net
fi
chmod +x ./kuula ./kuula-net 2>/dev/null

# The development receiver is networking. Without it there is nothing to
# start, and the person is told so instead of seeing the app flash by.
if [ "$1" = --dev ] && [ "$bin" != ./kuula-net ]; then
    why="Kuula Dev needs networking: a Mini Plus, and no file named offline in Emu/KUULA."
    echo "$why" > "$progdir/kuula.log"
    panel=/mnt/SDCARD/.tmp_update/bin/infoPanel
    [ -f "$panel" ] && "$panel" --title "Kuula Dev" --message "$why" --auto
    exit 0
fi

# The audio device has one owner. Onion's audio server gives it up here
# and Onion starts it again afterwards; the wrapper Onion preloads into
# what it launches is for programs that play through that server.
if [ -f /mnt/SDCARD/.tmp_update/script/stop_audioserver.sh ]; then
    . /mnt/SDCARD/.tmp_update/script/stop_audioserver.sh
fi
unset LD_PRELOAD

# A 60 Hz cart wants the clock fixed high; Onion sets it back afterwards.
echo performance > /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null

carts=/mnt/SDCARD/Roms/KUULA
case "$1" in
    "")
        $bin shell --carts "$carts" --in-process --scale 1
        ;;
    --dev)
        # The port is fixed so that the receiver's ticket, which this log
        # has on its first lines, stays the same from one start to the next.
        $bin shell --carts "$carts" --in-process --scale 1 --dev-receiver --bind 0.0.0.0:47756
        ;;
    *)
        $bin shell --cart "$1" --in-process --scale 1
        ;;
esac > "$progdir/kuula.log" 2>&1
status=$?
echo "kuula exited with status $status" >> "$progdir/kuula.log"
sync
exit $status
