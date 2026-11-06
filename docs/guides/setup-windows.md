# Setting up Pasta on Windows

Screenshots: to do (they need a build on a Windows PC; tracked with #45).

1. **Install Tailscale** from tailscale.com/download, sign in with the same account as
   your other devices, and connect.
2. **Open Pasta.** It lives in the system tray. The first time, setup opens.
3. **Connect Tailscale.** Setup checks Tailscale and says what to do if it isn't running
   or you're signed out (click the Tailscale icon in the taskbar and connect).
4. **Create or join a chain.** Choose **Join a chain** and type the 24 words from your
   other device (Settings › Devices › Add Device there), then approve this PC on that
   device after checking that both show the same key.
5. **Permissions & startup.**
   - **Clipboard history** must be on for items from your other devices to show in
     Win + V. If it's off, **Turn it on…** opens Settings › System › Clipboard.
   - **Sync across your devices** (Windows' own cloud clipboard): if it's on, setup warns
     that items may show up twice. Turn it off to let Pasta do the syncing.
   - **Firewall:** if Windows asks whether Pasta may use the network, allow it on private
     networks. Pasta only listens on the Tailscale interface.
   - **Keep Pasta in the taskbar:** drag its icon from the hidden icons (^) onto the
     taskbar so the flyout is one click away.
   - **Open at login** and **Update automatically** are up to you.
6. **All set.** Press Win + V to see items from your other devices. Click Pasta's tray
   icon for the flyout: status, mode, received items with their expiry, Pause for 1 hour,
   and Clear from Win + V.
