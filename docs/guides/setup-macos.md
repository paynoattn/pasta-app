# Setting up Pasta on a Mac

Screenshots: to do (they need a signed build on a real Mac; tracked with #44).

1. **Install Tailscale** from tailscale.com/download or the App Store, sign in, and connect.
   Use the same Tailscale account on every device you want to sync.
2. **Open Pasta.** It lives in the menu bar, with no Dock icon. The first time, setup opens.
3. **Connect Tailscale.** Setup checks that Tailscale is installed, running and signed in,
   and tells you what to do if it isn't.
4. **Create or join a chain.**
   - On your first device, choose **Create a new chain**. Pasta shows 24 words.
   - On the others, choose **Join a chain** and type those words. Then approve the new
     device on one that is already in the chain (Settings › Devices, or the request in
     Add Device). Check that both screens show the same key.
   - Only enter words you got from your own device. Never share them.
5. **Permissions & startup.**
   - **Paste into other apps:** click **Ask macOS** or **Open System Settings** and turn
     Pasta on under Privacy & Security › Accessibility. Setup notices as soon as you do.
     Without it, picking an item in the panel copies it and you press ⌘V yourself.
   - **Shortcut:** ⇧⌘V opens the clipboard panel. Many apps use ⇧⌘V for Paste and Match
     Style; while Pasta runs it takes the shortcut over. Change it in Settings ›
     Shortcuts, and use ⇧↩ in the panel to paste plain text.
   - **iCloud Universal Clipboard:** off by default. Turn it on to send copies from your
     iPhone or iPad to your other computers, and theirs to your iPhone.
   - **Open at login** and **Update automatically** are up to you.
6. **All set.** Press ⇧⌘V anywhere to open your clipboard history.
