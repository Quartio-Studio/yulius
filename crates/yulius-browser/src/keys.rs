//! DOM key names to Windows virtual key codes for `Input.dispatchKeyEvent`.
//!
//! Chromium needs `windowsVirtualKeyCode` for non-text keys (Backspace, Enter, arrows, …) to
//! take effect, and `text: "\r"` on Enter for it to submit forms.

/// The virtual key code for a DOM `key`/`code` pair, when known.
pub(crate) fn virtual_key_code(key: Option<&str>, code: Option<&str>) -> Option<u32> {
    key.and_then(from_key).or_else(|| code.and_then(from_code))
}

/// Text implied by a key that the viewer may not send (Enter produces `\r`).
pub(crate) fn implied_text(key: Option<&str>) -> Option<&'static str> {
    matches!(key, Some("Enter")).then_some("\r")
}

fn from_key(key: &str) -> Option<u32> {
    let named = match key {
        "Backspace" => 8,
        "Tab" => 9,
        "Enter" => 13,
        "Shift" => 16,
        "Control" => 17,
        "Alt" => 18,
        "Pause" => 19,
        "CapsLock" => 20,
        "Escape" => 27,
        " " => 32,
        "PageUp" => 33,
        "PageDown" => 34,
        "End" => 35,
        "Home" => 36,
        "ArrowLeft" => 37,
        "ArrowUp" => 38,
        "ArrowRight" => 39,
        "ArrowDown" => 40,
        "Insert" => 45,
        "Delete" => 46,
        "Meta" => 91,
        "ContextMenu" => 93,
        _ => 0,
    };
    if named != 0 {
        return Some(named);
    }
    if let Some(number) = key.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=24).contains(&number).then_some(111 + number);
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => Some(u32::from(c.to_ascii_uppercase())),
        _ => None,
    }
}

fn from_code(code: &str) -> Option<u32> {
    if let Some(letter) = code.strip_prefix("Key") {
        let mut chars = letter.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return c.is_ascii_uppercase().then_some(u32::from(c));
        }
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        let mut chars = digit.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return c.is_ascii_digit().then_some(u32::from(c));
        }
    }
    Some(match code {
        "Semicolon" => 186,
        "Equal" => 187,
        "Comma" => 188,
        "Minus" => 189,
        "Period" => 190,
        "Slash" => 191,
        "Backquote" => 192,
        "BracketLeft" => 219,
        "Backslash" => 220,
        "BracketRight" => 221,
        "Quote" => 222,
        "NumpadEnter" => 13,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_named_printable_and_code_keys() {
        assert_eq!(virtual_key_code(Some("Enter"), Some("Enter")), Some(13));
        assert_eq!(virtual_key_code(Some("Backspace"), None), Some(8));
        assert_eq!(virtual_key_code(Some("ArrowDown"), None), Some(40));
        assert_eq!(virtual_key_code(Some("F5"), None), Some(116));
        assert_eq!(virtual_key_code(Some("a"), Some("KeyA")), Some(65));
        assert_eq!(virtual_key_code(Some("7"), None), Some(55));
        assert_eq!(virtual_key_code(Some(";"), Some("Semicolon")), Some(186));
        assert_eq!(virtual_key_code(Some("ł"), Some("KeyL")), Some(76));
        assert_eq!(virtual_key_code(Some("Unidentified"), None), None);
        assert_eq!(virtual_key_code(None, None), None);
        assert_eq!(implied_text(Some("Enter")), Some("\r"));
        assert_eq!(implied_text(Some("a")), None);
    }
}
