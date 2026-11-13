//! KDE: puts the tray flyout next to the tray (#53). Wayland windows can't
//! place themselves, so a small KWin script moves Pasta's flyout into the
//! corner of the work area each time it appears: bottom right, or top right
//! when the panel is at the top. Loaded at start, unloaded on quit.

use std::path::PathBuf;

const PLUGIN: &str = "pasta-flyout";
/// Matches the flyout by Pasta's pid and the `tray` window's title.
const SCRIPT: &str = r#"
function place(window) {
  if (window.pid !== PASTA_PID || window.caption !== "Pasta") return;
  const area = workspace.clientArea(KWin.MaximizeArea, window);
  const screen = workspace.clientArea(KWin.FullScreenArea, window);
  const margin = 8;
  const top = area.y > screen.y;
  window.frameGeometry = {
    x: area.x + area.width - window.width - margin,
    y: top ? area.y + margin : area.y + area.height - window.height - margin,
    width: window.width,
    height: window.height,
  };
}
workspace.windowAdded.connect(place);
"#;

fn on_kde_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| desktop.split(':').any(|name| name == "KDE"))
}

pub fn install() {
    if !on_kde_wayland() {
        return;
    }
    tauri::async_runtime::spawn(async {
        if let Err(err) = load().await {
            tracing::warn!(error = %err, "couldn't place the tray flyout through KWin; it opens where KWin puts it");
        }
    });
}

pub async fn uninstall() {
    if on_kde_wayland()
        && let Ok(scripting) = scripting().await
    {
        let _ = scripting.call::<_, _, bool>("unloadScript", &(PLUGIN,)).await;
    }
}

async fn scripting() -> zbus::Result<zbus::Proxy<'static>> {
    let bus = zbus::Connection::session().await?;
    zbus::Proxy::new_owned(bus, "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting").await
}

async fn load() -> Result<(), String> {
    // Only the per-user runtime dir: a fixed name in a shared /tmp could be a planted symlink.
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).ok_or("XDG_RUNTIME_DIR is not set")?;
    let path = dir.join(format!("{PLUGIN}.js"));
    let script = SCRIPT.replace("PASTA_PID", &std::process::id().to_string());
    std::fs::write(&path, script).map_err(|err| err.to_string())?;
    let scripting = scripting().await.map_err(|err| err.to_string())?;
    // A copy left by a Pasta that didn't quit cleanly.
    let _ = scripting.call::<_, _, bool>("unloadScript", &(PLUGIN,)).await;
    let id: i32 = scripting
        .call("loadScript", &(path.to_string_lossy().as_ref(), PLUGIN))
        .await
        .map_err(|err| err.to_string())?;
    if id < 0 {
        return Err("KWin refused the script".into());
    }
    let script = zbus::Proxy::new(
        scripting.connection(),
        "org.kde.KWin",
        format!("/Scripting/Script{id}"),
        "org.kde.kwin.Script",
    )
    .await
    .map_err(|err| err.to_string())?;
    script.call::<_, _, ()>("run", &()).await.map_err(|err| err.to_string())
}
