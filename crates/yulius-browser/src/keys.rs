//! Key mapping.
//!
//! - Headless browsers (CDP input): DOM key names to Windows virtual key codes for
//!   `Input.dispatchKeyEvent`. Chromium needs `windowsVirtualKeyCode` for non-text keys
//!   (Backspace, Enter, arrows, …) to take effect, and `text: "\r"` on Enter for it to submit
//!   forms.
//! - Virtual displays (XTest, SPEC-002 ADR-15): DOM keys to X keycodes of the display's keymap
//!   (Xvfb's default: evdev keycodes, US layout). Printable ASCII is mapped by the character the
//!   user typed, with the Shift state it needs on a US layout, so any user layout types the
//!   right character and shortcuts follow the character (Ctrl+Z is Ctrl+Z on AZERTY too).
//!   Other keys are mapped by their physical `code`, which no layout changes. Characters a US
//!   layout cannot type (`ą`, `é`, emoji) are not keys: they are inserted as text.

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

/// How a key of a virtual display is typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DisplayKey {
    /// A printable ASCII character: its keycode on a US layout and whether Shift must be down.
    Char {
        /// X keycode.
        keycode: u8,
        /// The character needs Shift.
        shift: bool,
    },
    /// A non-text key (Enter, arrows, modifiers, F-keys): its X keycode.
    Key(u8),
    /// A character no US key types: inserted as text on key down.
    Text(String),
    /// Not forwarded: lock keys (the viewer's own state already shaped the characters), AltGraph
    /// (its characters arrive as text) and unknown keys.
    Ignore,
}

/// X keycodes of Shift (left, right).
pub(crate) const SHIFT_KEYCODES: [u8; 2] = [50, 62];
/// X keycodes of Ctrl, Alt and Meta (left and right).
pub(crate) const SHORTCUT_KEYCODES: [u8; 6] = [37, 105, 64, 108, 133, 134];

/// Maps a viewer key event to the display.
pub(crate) fn display_key(key: Option<&str>, code: Option<&str>) -> DisplayKey {
    let key = key.unwrap_or_default();
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && !c.is_control()
    {
        return match us_char(c) {
            Some((keycode, shift)) => DisplayKey::Char { keycode, shift },
            None => DisplayKey::Text(c.to_string()),
        };
    }
    if matches!(
        key,
        "CapsLock" | "NumLock" | "ScrollLock" | "AltGraph" | "Dead" | "Process" | "Compose"
    ) {
        return DisplayKey::Ignore;
    }
    code.and_then(code_keycode)
        .or_else(|| named_keycode(key))
        .map_or(DisplayKey::Ignore, DisplayKey::Key)
}

/// The keycode and Shift state that type `c` on a US layout (evdev keycodes).
pub(crate) fn us_char(c: char) -> Option<(u8, bool)> {
    const ROW_DIGITS: &[u8; 10] = b"1234567890";
    const ROW_SHIFTED: &[u8; 10] = b"!@#$%^&*()";
    const LETTERS: &[(u8, u8)] = &[
        (b'q', 24),
        (b'w', 25),
        (b'e', 26),
        (b'r', 27),
        (b't', 28),
        (b'y', 29),
        (b'u', 30),
        (b'i', 31),
        (b'o', 32),
        (b'p', 33),
        (b'a', 38),
        (b's', 39),
        (b'd', 40),
        (b'f', 41),
        (b'g', 42),
        (b'h', 43),
        (b'j', 44),
        (b'k', 45),
        (b'l', 46),
        (b'z', 52),
        (b'x', 53),
        (b'c', 54),
        (b'v', 55),
        (b'b', 56),
        (b'n', 57),
        (b'm', 58),
    ];
    // (unshifted, shifted, keycode)
    const PUNCTUATION: &[(u8, u8, u8)] = &[
        (b'`', b'~', 49),
        (b'-', b'_', 20),
        (b'=', b'+', 21),
        (b'[', b'{', 34),
        (b']', b'}', 35),
        (b'\\', b'|', 51),
        (b';', b':', 47),
        (b'\'', b'"', 48),
        (b',', b'<', 59),
        (b'.', b'>', 60),
        (b'/', b'?', 61),
    ];
    let byte = u8::try_from(c)
        .ok()
        .filter(u8::is_ascii_graphic)
        .or((c == ' ').then_some(b' '))?;
    if byte == b' ' {
        return Some((65, false));
    }
    if let Some(index) = ROW_DIGITS.iter().position(|d| *d == byte) {
        return Some((10 + u8::try_from(index).ok()?, false));
    }
    if let Some(index) = ROW_SHIFTED.iter().position(|d| *d == byte) {
        return Some((10 + u8::try_from(index).ok()?, true));
    }
    let lower = byte.to_ascii_lowercase();
    if let Some((_, keycode)) = LETTERS.iter().find(|(letter, _)| *letter == lower) {
        return Some((*keycode, byte.is_ascii_uppercase()));
    }
    PUNCTUATION.iter().find_map(|(plain, shifted, keycode)| {
        (*plain == byte)
            .then_some((*keycode, false))
            .or((*shifted == byte).then_some((*keycode, true)))
    })
}

/// The X keycode of a physical key (`KeyboardEvent.code`).
pub(crate) fn code_keycode(code: &str) -> Option<u8> {
    if let Some(letter) = code.strip_prefix("Key") {
        let mut chars = letter.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return c
                .is_ascii_uppercase()
                .then(|| us_char(c.to_ascii_lowercase()))
                .flatten()
                .map(|(keycode, _)| keycode);
        }
        return None;
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        let mut chars = digit.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return c
                .is_ascii_digit()
                .then(|| us_char(c))
                .flatten()
                .map(|(k, _)| k);
        }
        return None;
    }
    if let Some(number) = code.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        return match number {
            1..=10 => Some(66 + number),
            11 | 12 => Some(84 + number),
            13..=24 => Some(178 + number),
            _ => None,
        };
    }
    Some(match code {
        "Escape" => 9,
        "Minus" => 20,
        "Equal" => 21,
        "Backspace" => 22,
        "Tab" => 23,
        "BracketLeft" => 34,
        "BracketRight" => 35,
        "Enter" => 36,
        "ControlLeft" => 37,
        "Semicolon" => 47,
        "Quote" => 48,
        "Backquote" => 49,
        "ShiftLeft" => 50,
        "Backslash" => 51,
        "Comma" => 59,
        "Period" => 60,
        "Slash" => 61,
        "ShiftRight" => 62,
        "NumpadMultiply" => 63,
        "AltLeft" => 64,
        "Space" => 65,
        "Numpad7" => 79,
        "Numpad8" => 80,
        "Numpad9" => 81,
        "NumpadSubtract" => 82,
        "Numpad4" => 83,
        "Numpad5" => 84,
        "Numpad6" => 85,
        "NumpadAdd" => 86,
        "Numpad1" => 87,
        "Numpad2" => 88,
        "Numpad3" => 89,
        "Numpad0" => 90,
        "NumpadDecimal" => 91,
        "IntlBackslash" => 94,
        "NumpadEnter" => 104,
        "ControlRight" => 105,
        "NumpadDivide" => 106,
        "PrintScreen" => 107,
        "AltRight" => 108,
        "Home" => 110,
        "ArrowUp" => 111,
        "PageUp" => 112,
        "ArrowLeft" => 113,
        "ArrowRight" => 114,
        "End" => 115,
        "ArrowDown" => 116,
        "PageDown" => 117,
        "Insert" => 118,
        "Delete" => 119,
        "Pause" => 127,
        "MetaLeft" => 133,
        "MetaRight" => 134,
        "ContextMenu" => 135,
        _ => return None,
    })
}

/// The X keycode of a named key when the viewer sent no usable `code` (virtual keyboards).
fn named_keycode(key: &str) -> Option<u8> {
    let code = match key {
        "Shift" => "ShiftLeft",
        "Control" => "ControlLeft",
        "Alt" => "AltLeft",
        "Meta" | "OS" => "MetaLeft",
        "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "Home" | "End" | "PageUp"
        | "PageDown" | "Insert" | "Delete" | "Backspace" | "Tab" | "Enter" | "Escape"
        | "ContextMenu" | "Pause" | "PrintScreen" => key,
        _ if key.starts_with('F') => key,
        _ => return None,
    };
    code_keycode(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_ascii_follows_the_character_on_a_us_layout() {
        let char_key = |key: &str, code: &str| display_key(Some(key), Some(code));
        assert_eq!(
            char_key("a", "KeyA"),
            DisplayKey::Char {
                keycode: 38,
                shift: false
            }
        );
        assert_eq!(
            char_key("A", "KeyA"),
            DisplayKey::Char {
                keycode: 38,
                shift: true
            }
        );
        // AZERTY: the physical KeyQ types "a"; the character decides.
        assert_eq!(
            char_key("a", "KeyQ"),
            DisplayKey::Char {
                keycode: 38,
                shift: false
            }
        );
        // German "z" sits on KeyY.
        assert_eq!(
            char_key("z", "KeyY"),
            DisplayKey::Char {
                keycode: 52,
                shift: false
            }
        );
        assert_eq!(
            char_key("!", "Digit1"),
            DisplayKey::Char {
                keycode: 10,
                shift: true
            }
        );
        // AZERTY types "!" without Shift on another key: Shift is added for the US key.
        assert_eq!(
            char_key("!", "Slash"),
            DisplayKey::Char {
                keycode: 10,
                shift: true
            }
        );
        assert_eq!(
            char_key("0", "Digit0"),
            DisplayKey::Char {
                keycode: 19,
                shift: false
            }
        );
        assert_eq!(
            char_key("?", "Slash"),
            DisplayKey::Char {
                keycode: 61,
                shift: true
            }
        );
        assert_eq!(
            char_key("\\", "Backslash"),
            DisplayKey::Char {
                keycode: 51,
                shift: false
            }
        );
        assert_eq!(
            char_key("\"", "Quote"),
            DisplayKey::Char {
                keycode: 48,
                shift: true
            }
        );
        assert_eq!(
            char_key(" ", "Space"),
            DisplayKey::Char {
                keycode: 65,
                shift: false
            }
        );
        assert_eq!(
            char_key("~", "Backquote"),
            DisplayKey::Char {
                keycode: 49,
                shift: true
            }
        );
    }

    #[test]
    fn characters_without_a_us_key_are_text() {
        for (key, code) in [
            ("ą", "KeyA"),
            ("ł", "KeyL"),
            ("é", "Digit2"),
            ("€", "KeyE"),
            ("😀", ""),
        ] {
            assert_eq!(
                display_key(Some(key), Some(code)),
                DisplayKey::Text(key.into()),
                "{key}"
            );
        }
    }

    #[test]
    fn other_keys_follow_the_physical_code() {
        let key = |key: &str, code: &str| display_key(Some(key), Some(code));
        assert_eq!(key("Enter", "Enter"), DisplayKey::Key(36));
        assert_eq!(key("Enter", "NumpadEnter"), DisplayKey::Key(104));
        assert_eq!(key("Backspace", "Backspace"), DisplayKey::Key(22));
        assert_eq!(key("ArrowLeft", "ArrowLeft"), DisplayKey::Key(113));
        assert_eq!(key("Shift", "ShiftRight"), DisplayKey::Key(62));
        assert_eq!(key("Control", "ControlLeft"), DisplayKey::Key(37));
        assert_eq!(key("Meta", "MetaLeft"), DisplayKey::Key(133));
        assert_eq!(key("F1", "F1"), DisplayKey::Key(67));
        assert_eq!(key("F10", "F10"), DisplayKey::Key(76));
        assert_eq!(key("F12", "F12"), DisplayKey::Key(96));
        assert_eq!(key("F13", "F13"), DisplayKey::Key(191));
        assert_eq!(key("Escape", "Escape"), DisplayKey::Key(9));
        assert_eq!(key("Delete", "Delete"), DisplayKey::Key(119));
        // A virtual keyboard without codes.
        assert_eq!(display_key(Some("Backspace"), None), DisplayKey::Key(22));
        assert_eq!(display_key(Some("Shift"), Some("")), DisplayKey::Key(50));
    }

    #[test]
    fn lock_keys_altgraph_and_unknown_keys_are_not_forwarded() {
        for (key, code) in [
            ("CapsLock", "CapsLock"),
            ("NumLock", "NumLock"),
            ("AltGraph", "AltRight"),
            ("Dead", "BracketLeft"),
            ("Process", "KeyA"),
            ("Unidentified", ""),
            ("BrightnessUp", "BrightnessUp"),
        ] {
            assert_eq!(
                display_key(Some(key), Some(code)),
                DisplayKey::Ignore,
                "{key}"
            );
        }
        assert_eq!(display_key(None, None), DisplayKey::Ignore);
    }

    #[test]
    fn every_us_character_round_trips_to_a_distinct_key_state() {
        let mut seen = std::collections::HashSet::new();
        for byte in 0x20_u8..0x7F {
            let c = char::from(byte);
            let mapped = us_char(c);
            assert!(mapped.is_some(), "{c:?} has a US key");
            assert!(seen.insert(mapped), "{c:?} is unique");
        }
        assert_eq!(us_char('\n'), None);
        assert_eq!(us_char('ą'), None);
    }

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
