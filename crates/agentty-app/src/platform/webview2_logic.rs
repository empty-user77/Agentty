//! The parts of the Windows in-app browser (`platform/windows/webview.rs`) that are plain logic:
//! building scripts, reading the DevTools protocol's answers, telling shortcuts apart. Kept apart
//! from the WebView2 calls so they are compiled and tested on every platform, not only on Windows.

use crate::webview::BrowserKey;
use serde_json::Value;

/// The function WebKit's `callAsyncJavaScript` would make of `body` with `args` as its
/// parameters, as one expression for `Runtime.evaluate`. Only a string comes back, anything else
/// is `null` (as WebKit's answers are read on macOS): a DOM node or a cyclic object returned by a
/// script must not fail the call.
pub fn script_of(body: &str, args: &[(&str, &str)]) -> Result<String, String> {
    let valid = |name: &str| {
        let mut chars = name.chars();
        chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
    };
    if let Some((name, _)) = args.iter().find(|(name, _)| !valid(name)) {
        return Err(format!("invalid argument name {name:?}"));
    }
    let names: Vec<&str> = args.iter().map(|(name, _)| *name).collect();
    let values: Vec<String> = args.iter().map(|(_, value)| Value::String(value.to_string()).to_string()).collect();
    Ok(format!(
        "(async function ({}) {{\n{body}\n}})({}).then((value) => typeof value === 'string' ? value : null)",
        names.join(", "),
        values.join(", ")
    ))
}

/// What a `Runtime.evaluate` result means for the caller: the string the script returned,
/// `null` for anything else, or the error it threw (its message: the first line of the stack).
pub fn evaluate_answer(value: &Value) -> Result<String, String> {
    if let Some(details) = value.get("exceptionDetails") {
        let text = details
            .pointer("/exception/description")
            .and_then(Value::as_str)
            .or_else(|| details.pointer("/exception/value").and_then(Value::as_str))
            .or_else(|| details.get("text").and_then(Value::as_str))
            .unwrap_or("JavaScript error");
        return Err(text.lines().next().unwrap_or(text).to_string());
    }
    Ok(match value.pointer("/result/value") {
        Some(Value::String(text)) => text.clone(),
        _ => "null".into(),
    })
}

/// Whether a script's error says its world's document is gone (the script never ran, so it may
/// be sent again in a new world). "Execution context was destroyed" — the page navigated while
/// the script ran — is not: it may have done part of its work.
pub fn world_is_gone(error: &str) -> bool {
    error.contains("Cannot find context")
}

/// The PNG in a `Page.captureScreenshot` answer.
pub fn screenshot_png(value: &Value) -> Result<Vec<u8>, String> {
    use base64::Engine;
    let data = value.get("data").and_then(Value::as_str).ok_or("snapshot failed")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| "snapshot failed".to_string())?;
    if !bytes.starts_with(b"\x89PNG") {
        return Err("snapshot failed".into());
    }
    Ok(bytes)
}

/// The DevTools protocol's error message in an answer that failed, when it has one.
pub fn devtools_error(json: &str) -> Option<String> {
    serde_json::from_str::<Value>(json).ok()?.get("message")?.as_str().map(str::to_string)
}

/// Which browser shortcut a key down in a page is: virtual-key code and the modifiers held.
pub fn browser_key_for(key: u32, control: bool, shift: bool, alt: bool) -> Option<BrowserKey> {
    const LEFT: u32 = 0x25;
    const RIGHT: u32 = 0x27;
    const F5: u32 = 0x74;
    match (key, control, shift, alt) {
        (F5, false, false, false) => Some(BrowserKey::Reload),
        (F5, true, false, false) => Some(BrowserKey::HardReload),
        (LEFT, false, false, true) => Some(BrowserKey::Back),
        (RIGHT, false, false, true) => Some(BrowserKey::Forward),
        // Letters only: virtual-key codes 0x60–0x7A are the numpad and F1–F11, not `a`–`z`.
        (0x41..=0x5A, true, _, false) => match (char::from_u32(key).map(|c| c.to_ascii_lowercase()), shift) {
            (Some('r'), true) => Some(BrowserKey::HardReload),
            (Some('r'), false) => Some(BrowserKey::Reload),
            (Some('t'), false) => Some(BrowserKey::NewTab),
            (Some('w'), false) => Some(BrowserKey::CloseTab),
            (Some('l'), false) => Some(BrowserKey::FocusAddress),
            _ => None,
        },
        _ => None,
    }
}

/// Whether an Authenticode signer's subject (`CN=Microsoft Corporation, O=Microsoft Corporation,
/// L=Redmond, …`) names Microsoft as the organization.
pub fn signed_by_microsoft(subject: &str) -> bool {
    subject.split(',').any(|part| part.trim() == "O=Microsoft Corporation")
}

/// A profile's name in WebView2 (a folder of its own under the data folder): letters, digits and
/// `-`, well within its 64 characters.
pub fn profile_name(bytes: &[u8; 16]) -> String {
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("agentty-{hex}")
}

/// The major version of a WebView2 Runtime version string (`128.0.2739.42` → 128).
pub fn major_version(version: &str) -> u32 {
    version.split('.').next().and_then(|major| major.trim().parse().ok()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scripts_take_their_arguments_as_parameters() {
        let script = script_of("return a + b;", &[("a", "1\"x"), ("b", "</script>")]).unwrap();
        assert_eq!(
            script,
            "(async function (a, b) {\nreturn a + b;\n})(\"1\\\"x\", \"</script>\").then((value) => typeof value === 'string' ? value : null)"
        );
        assert_eq!(
            script_of("return 1;", &[]).unwrap(),
            "(async function () {\nreturn 1;\n})().then((value) => typeof value === 'string' ? value : null)"
        );
        // What the app passes: agents' selectors, a plugin's input.
        assert!(script_of("", &[("selector", "#a"), ("__agenttyInput", "{}"), ("$x", "")]).is_ok());
        assert!(script_of("", &[("a b", "")]).is_err());
        assert!(script_of("", &[("1a", "")]).is_err());
        assert!(script_of("", &[("a);alert(1", "")]).is_err());
        assert!(script_of("", &[("", "")]).is_err());
    }

    #[test]
    fn a_line_comment_at_the_end_of_a_body_does_not_swallow_the_call() {
        // The body is closed on a line of its own: `// …` at its end cannot comment out `})(…)`.
        let script = script_of("return 'x' // done", &[]).unwrap();
        assert!(script.contains("// done\n})()"));
    }

    #[test]
    fn evaluate_answers_read_like_webkits() {
        assert_eq!(evaluate_answer(&json!({ "result": { "type": "string", "value": "{\"a\":1}" } })), Ok("{\"a\":1}".into()));
        assert_eq!(evaluate_answer(&json!({ "result": { "type": "string", "value": "" } })), Ok("".into()));
        assert_eq!(evaluate_answer(&json!({ "result": { "type": "object", "subtype": "null", "value": null } })), Ok("null".into()));
        assert_eq!(evaluate_answer(&json!({ "result": { "type": "undefined" } })), Ok("null".into()));
        let thrown = json!({ "result": {}, "exceptionDetails": { "text": "Uncaught", "exception": {
            "description": "Error: no element matches #go\n    at <anonymous>:3:9" } } });
        assert_eq!(evaluate_answer(&thrown), Err("Error: no element matches #go".into()));
        let syntax =
            json!({ "exceptionDetails": { "text": "Uncaught", "exception": { "description": "SyntaxError: Unexpected token 'return'" } } });
        assert!(evaluate_answer(&syntax).unwrap_err().contains("SyntaxError"), "eval's statement retry looks for it");
        let rejected =
            json!({ "exceptionDetails": { "text": "Uncaught (in promise)", "exception": { "type": "string", "value": "timed out" } } });
        assert_eq!(evaluate_answer(&rejected), Err("timed out".into()));
        assert_eq!(evaluate_answer(&json!({ "exceptionDetails": {} })), Err("JavaScript error".into()));
    }

    #[test]
    fn only_a_missing_world_is_tried_again() {
        assert!(world_is_gone("Cannot find context with specified id"));
        assert!(!world_is_gone("Execution context was destroyed."));
        assert!(!world_is_gone("Error: context menu not found"));
    }

    #[test]
    fn screenshots_are_pngs() {
        use base64::Engine;
        let png = b"\x89PNG\r\n\x1a\nrest";
        let data = base64::engine::general_purpose::STANDARD.encode(png);
        assert_eq!(screenshot_png(&json!({ "data": data })).unwrap(), png.to_vec());
        assert!(screenshot_png(&json!({ "data": "not base64!" })).is_err());
        assert!(screenshot_png(&json!({ "data": base64::engine::general_purpose::STANDARD.encode(b"GIF89a") })).is_err());
        assert!(screenshot_png(&json!({})).is_err());
        assert_eq!(
            devtools_error(r#"{"code":-32000,"message":"Cannot find context with specified id"}"#).as_deref(),
            Some("Cannot find context with specified id")
        );
        assert_eq!(devtools_error("not json"), None);
    }

    #[test]
    fn browser_shortcuts_are_told_apart() {
        let key = |c: char| c as u32;
        assert_eq!(browser_key_for(key('R'), true, false, false), Some(BrowserKey::Reload));
        assert_eq!(browser_key_for(key('R'), true, true, false), Some(BrowserKey::HardReload));
        assert_eq!(browser_key_for(key('T'), true, false, false), Some(BrowserKey::NewTab));
        assert_eq!(browser_key_for(key('W'), true, false, false), Some(BrowserKey::CloseTab));
        assert_eq!(browser_key_for(key('L'), true, false, false), Some(BrowserKey::FocusAddress));
        assert_eq!(browser_key_for(0x74, false, false, false), Some(BrowserKey::Reload));
        assert_eq!(browser_key_for(0x74, true, false, false), Some(BrowserKey::HardReload));
        assert_eq!(browser_key_for(0x25, false, false, true), Some(BrowserKey::Back));
        assert_eq!(browser_key_for(0x27, false, false, true), Some(BrowserKey::Forward));
        // Typing, copying and the page's own shortcuts stay the page's.
        assert_eq!(browser_key_for(key('R'), false, false, false), None);
        assert_eq!(browser_key_for(key('C'), true, false, false), None);
        assert_eq!(browser_key_for(key('V'), true, false, false), None);
        assert_eq!(browser_key_for(key('T'), true, true, false), None);
        // AltGr (Ctrl+Alt) types characters on many keyboards.
        assert_eq!(browser_key_for(key('R'), true, false, true), None);
        // Function keys and the numpad share codes with lowercase letters: F3 is 0x72 ('r').
        assert_eq!(browser_key_for(0x72, true, false, false), None);
        assert_eq!(browser_key_for(0x77, true, false, false), None);
        assert_eq!(browser_key_for(0x6C, true, false, false), None);
        assert_eq!(browser_key_for(0x25, false, false, false), None);
    }

    #[test]
    fn only_microsoft_signs_the_runtime_installer() {
        assert!(signed_by_microsoft("CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US"));
        assert!(!signed_by_microsoft("CN=Microsoft Corporation, O=Evil Microsoft Corporation Ltd"));
        assert!(!signed_by_microsoft("CN=O=Microsoft Corporation"));
        assert!(!signed_by_microsoft(""));
    }

    #[test]
    fn profile_names_are_what_webview2_accepts() {
        let name = profile_name(&[0xab; 16]);
        assert_eq!(name, format!("agentty-{}", "ab".repeat(16)));
        assert!(name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        assert_eq!(major_version("128.0.2739.42"), 128);
        assert_eq!(major_version(""), 0);
        assert_eq!(major_version("x.1"), 0);
    }
}
