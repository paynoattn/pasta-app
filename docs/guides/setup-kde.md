# Setting up Pasta on KDE Plasma

Screenshots: to do (tracked with #54). Pasta needs Plasma 6 in a **Wayland** session; it
can't watch the clipboard on X11.

1. **Install Tailscale** (`sudo pacman -S tailscale` on Arch / CachyOS), then enable and
   connect it, signing in with the same account as your other devices:

   ```sh
   sudo systemctl enable --now tailscaled
   sudo tailscale up
   ```

2. **Install Pasta.** On Arch / CachyOS, from the AUR (`pasta-bin`), or build a checkout
   with `packaging/arch/PKGBUILD`. Other distributions: the .deb or AppImage from the
   release. Then open **Pasta** from the app launcher. It lives in the system tray; the
   first time, setup opens.
3. **Connect Tailscale.** Setup checks Tailscale and says what to do if it isn't running
   or you're signed out. If it says access was denied, let your user talk to Tailscale
   once: `sudo tailscale set --operator=$USER`.
4. **Create or join a chain.** Choose **Join a chain** and type the 24 words from your
   other device (Settings › Devices › Add Device there), then approve this machine on
   that device after checking that both show the same key.
5. **Permissions & startup.**
   - **Klipper** must be running for items from your other devices to show up in its
     history (Meta + V). It's part of Plasma and on by default.
   - **Keep Pasta in the tray:** right-click the tray arrow › **Configure System Tray…** ›
     Entries, and set Pasta to **Always shown**, so the flyout is one click away.
   - Pasta's keys go in KWallet (any Secret Service provider works). If KWallet asks
     whether Pasta may open the wallet, allow it.
   - **Open at login** and **Update automatically** are up to you.
6. **All set.** Press Meta + V to see items from your other devices in Klipper. Click
   Pasta's tray icon for the flyout: status, mode, received items with their expiry,
   and Pause for 1 hour. **Settings › Clipboard › Expire all now** clears every received
   item from the clipboard at once.

## Expiry and Klipper

Klipper has no way for another app to delete a single entry. When an item expires,
Pasta clears it from the clipboard, but with **Settings › Clipboard › Add received items
to Klipper** on, Klipper keeps its copy in history. Turn that off and received items
only become the current clipboard, so expiry removes them completely.

Password managers that mark copied secrets (KeePassXC does) are never synced. Wayland
doesn't tell Pasta which app copied something, so there's no excluded-apps list on
Plasma.
