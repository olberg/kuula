Kuula for the Miyoo Mini (Plus), OnionOS
========================================

Install
  1. Unzip Kuula-Miyoo-<version>.zip into the root of the SD card. It only
     adds files under Roms/PORTS.
  2. On the device: Ports, then run "~Import ports" (or Refresh) once. The
     entries "Kuula" and "Kuula Hello" appear under "Fantasy consoles".

Run
  Kuula        the shell: pick a cart, A to run it.
  Kuula Hello  examples/hello on its own, the first thing to try.
  Hold Menu for a second and a half to leave. Inside the shell, Menu opens
  the pause menu.

Buttons
  D-pad        the arrows
  A            A
  B            B
  Menu         Menu (hold to exit)
  Other buttons are not used.

Check without a screen
  Over SSH:  cd /mnt/SDCARD/Roms/PORTS/Games/Kuula && sh probe.sh
  It runs the conformance carts headless and compares them with the
  hashes the desktop build printed. Output is also in probe.log. If a
  cart will not start, kuula.log has the reason.

This build has no networking. Saves and settings are in data/ beside the
binary. The libraries in libs/ are steward-fu's SDL2 for the Miyoo Mini
(zlib/LGPL, source at github.com/steward-fu/sdl2) and support libraries.
