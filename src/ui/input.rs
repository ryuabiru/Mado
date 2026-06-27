use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

pub fn key_to_nvim(event: &KeyEvent, modifiers: ModifiersState) -> Option<String> {
    if event.state != ElementState::Pressed {
        return None;
    }

    match &event.logical_key {
        Key::Character(text) => character_key(text, event.text.as_deref(), modifiers),
        Key::Named(named) => named_key(*named, modifiers),
        Key::Dead(_) | Key::Unidentified(_) => None,
    }
}

pub fn nvim_modifiers(modifiers: ModifiersState) -> String {
    let mut result = String::new();
    if modifiers.shift_key() {
        result.push('S');
    }
    if modifiers.control_key() {
        result.push('C');
    }
    if modifiers.alt_key() {
        result.push('A');
    }
    if modifiers.super_key() {
        result.push('D');
    }
    result
}

fn character_key(
    logical_text: &str,
    committed_text: Option<&str>,
    modifiers: ModifiersState,
) -> Option<String> {
    let has_command_modifier =
        modifiers.control_key() || modifiers.alt_key() || modifiers.super_key();
    if !has_command_modifier {
        let text = committed_text.filter(|text| !text.is_empty())?;
        return Some(text.replace('<', "<LT>"));
    }
    if logical_text.is_empty() {
        return None;
    }

    let mut names = Vec::new();
    if modifiers.control_key() {
        names.push("C");
    }
    if modifiers.alt_key() {
        names.push("A");
    }
    if modifiers.super_key() {
        names.push("D");
    }
    if modifiers.shift_key() {
        names.push("S");
    }
    Some(format!(
        "<{}-{}>",
        names.join("-"),
        logical_text.to_lowercase()
    ))
}

fn named_key(key: NamedKey, modifiers: ModifiersState) -> Option<String> {
    let name = match key {
        NamedKey::Escape => "Esc".to_owned(),
        NamedKey::Enter => "CR".to_owned(),
        NamedKey::Tab => "Tab".to_owned(),
        NamedKey::Backspace => "BS".to_owned(),
        NamedKey::Delete => "Del".to_owned(),
        NamedKey::Insert => "Insert".to_owned(),
        NamedKey::ArrowUp => "Up".to_owned(),
        NamedKey::ArrowDown => "Down".to_owned(),
        NamedKey::ArrowLeft => "Left".to_owned(),
        NamedKey::ArrowRight => "Right".to_owned(),
        NamedKey::Home => "Home".to_owned(),
        NamedKey::End => "End".to_owned(),
        NamedKey::PageUp => "PageUp".to_owned(),
        NamedKey::PageDown => "PageDown".to_owned(),
        NamedKey::F1 => "F1".to_owned(),
        NamedKey::F2 => "F2".to_owned(),
        NamedKey::F3 => "F3".to_owned(),
        NamedKey::F4 => "F4".to_owned(),
        NamedKey::F5 => "F5".to_owned(),
        NamedKey::F6 => "F6".to_owned(),
        NamedKey::F7 => "F7".to_owned(),
        NamedKey::F8 => "F8".to_owned(),
        NamedKey::F9 => "F9".to_owned(),
        NamedKey::F10 => "F10".to_owned(),
        NamedKey::F11 => "F11".to_owned(),
        NamedKey::F12 => "F12".to_owned(),
        NamedKey::Space => "Space".to_owned(),
        _ => return None,
    };
    let modifiers = nvim_modifiers(modifiers);
    if modifiers.is_empty() {
        Some(format!("<{name}>"))
    } else {
        let joined = modifiers
            .chars()
            .map(|modifier| modifier.to_string())
            .collect::<Vec<_>>()
            .join("-");
        Some(format!("<{joined}-{name}>"))
    }
}

#[cfg(test)]
mod tests {
    use super::character_key;
    use winit::keyboard::ModifiersState;

    #[test]
    fn escapes_literal_angle_bracket() {
        assert_eq!(
            character_key("<", Some("<"), ModifiersState::empty()).as_deref(),
            Some("<LT>")
        );
    }

    #[test]
    fn encodes_control_character() {
        assert_eq!(
            character_key("S", None, ModifiersState::CONTROL).as_deref(),
            Some("<C-s>")
        );
    }

    #[test]
    fn does_not_forward_uncommitted_ime_key() {
        assert_eq!(character_key("k", None, ModifiersState::empty()), None);
    }
}
