Kuula for the Miyoo Mini and Mini Plus, OnionOS
===============================================

Install
  1. Unzip Kuula-Miyoo-<version>.zip into the root of the SD card. It adds
     Emu/KUULA, Roms/KUULA, App/Kuula and App/KuulaDev, and nothing else.
  2. On the device: Games, then SELECT and "Refresh all roms". "Kuula"
     appears in the Games list, and "Kuula" and "Kuula Dev" under Apps.

Play
  Games > Kuula   the carts in Roms/KUULA. A cart is one .cart file; put
                  your own there and refresh the list.
  Apps > Kuula    Kuula's own list of the same carts, and the multiplayer
                  screens for a cart that plays over the network.
  In a cart, Menu opens the pause menu: resume, restart, settings, quit.
  Holding Menu for a second and a half leaves from anywhere.

Buttons
  D-pad        the arrows
  A            A
  B            B
  Menu         the pause menu (hold to leave)
  X, Y, L1, R1, L2, R2, Start and Select are a cart's when it asks for
  them; the Buttons test shows each. Start and Select together are Menu,
  and either alone is in a cart that does not use them.

Network (Mini Plus)
  A Mini Plus runs the build with networking; an original Mini, which has
  no Wi-Fi, runs the one without. To run without it on a Plus too, put an
  empty file named "offline" into Emu/KUULA.
  Networking is off until you allow it: a cart that plays over the network
  asks on its multiplayer screen. On the same Wi-Fi a session is joined
  from "nearby sessions", with no ticket to type.

Development (Mini Plus)
  Apps > Kuula Dev receives carts from a computer:
    kuula deploy push <cart directory> --to <ticket>
  The ticket is on the first lines of Emu/KUULA/kuula.log while the app
  runs, and stays the same as long as the device keeps its address. The
  first push from a computer is refused, and the device asks whether to
  approve it, naming the cart, its size and the sender's address: hold A
  for a second to approve, and the next push is installed and started.

Check without a screen
  Over SSH:  cd /mnt/SDCARD/Emu/KUULA && sh probe.sh
  It runs the conformance carts headless with each binary and compares
  them with the hashes the desktop build printed. Output is also in
  probe.log. If a cart will not start, kuula.log has the reason.

Saves and settings are in Saves/CurrentProfile/saves/Kuula. The libraries
in Emu/KUULA/libs are steward-fu's SDL2 for the Miyoo Mini (zlib/LGPL,
source at github.com/steward-fu/sdl2) and support libraries.

An earlier version of this package installed into Roms/PORTS/Games/Kuula;
that folder and Roms/PORTS/Shortcuts/Fantasy consoles can be deleted.
