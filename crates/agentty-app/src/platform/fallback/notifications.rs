//! Desktop notifications on Windows (toast through PowerShell's WinRT bridge) and Linux
//! (`notify-send`, the freedesktop notification service). Best effort: when neither is
//! available nothing is shown, as with unbundled macOS builds without `osascript`.

pub fn prepare() {}

pub fn show(_pane_id: u64, title: &str, body: &str) {
    let (title, body) = (clean(title, 120), clean(body, 300));
    std::thread::spawn(move || {
        #[cfg(windows)]
        let _ = agentty_bridge::process::windows_powershell()
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-EncodedCommand",
                &crate::launch::encode_powershell(&toast_script(&title, &body)),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        #[cfg(not(windows))]
        let _ = std::process::Command::new("notify-send")
            .args(["--app-name=Agentty", "--icon=utilities-terminal", &title, &body])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// Toasts and `notify-send` notifications are not taken back here: they expire on their own.
pub fn withdraw(_pane_id: u64) {}

/// Printable text only, bounded in length.
fn clean(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(max).collect()
}

/// A toast with Agentty's name, shown through PowerShell's registered app id (unpackaged apps have
/// none of their own).
#[cfg_attr(not(windows), allow(dead_code))]
fn toast_script(title: &str, body: &str) -> String {
    let escape =
        |text: &str| text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "''");
    format!(
        "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null\n\
         [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] > $null\n\
         $xml = New-Object Windows.Data.Xml.Dom.XmlDocument\n\
         $xml.LoadXml('<toast><visual><binding template=\"ToastGeneric\"><text>Agentty · {}</text><text>{}</text></binding></visual></toast>')\n\
         $id = '{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe'\n\
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($id).Show([Windows.UI.Notifications.ToastNotification]::new($xml))\n",
        escape(title),
        escape(body)
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn toast_text_is_escaped() {
        let script = super::toast_script("Build <done>", "it's \"ok\" & fine");
        assert!(script.contains("Build &lt;done&gt;"));
        assert!(script.contains("it''s &quot;ok&quot; &amp; fine"));
    }
}
