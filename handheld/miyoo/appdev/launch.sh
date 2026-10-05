#!/bin/sh
# App/KuulaDev: the shell as a development receiver. A cart pushed with
# `kuula deploy push` from an approved computer is installed into
# Roms/KUULA and started; a computer nobody has approved is asked about on
# the screen. The receiver's ticket is on the first lines of
# /mnt/SDCARD/Emu/KUULA/kuula.log.
exec /mnt/SDCARD/Emu/KUULA/launch.sh --dev
