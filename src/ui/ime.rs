#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ImeState {
    #[default]
    Idle,
    Composing {
        text: String,
        cursor: Option<(usize, usize)>,
    },
    AwaitingCommit,
}

impl ImeState {
    pub fn set_preedit(&mut self, text: String, cursor: Option<(usize, usize)>) {
        if text.is_empty() {
            *self = if matches!(self, Self::Composing { .. }) {
                Self::AwaitingCommit
            } else {
                Self::Idle
            };
        } else {
            *self = Self::Composing { text, cursor };
        }
    }

    pub fn commit(&mut self, text: String) -> Option<String> {
        *self = Self::Idle;
        (!text.is_empty()).then_some(text)
    }

    pub fn cancel(&mut self) {
        *self = Self::Idle;
    }

    pub fn blocks_keyboard_input(&mut self) -> bool {
        match self {
            Self::Composing { .. } => true,
            Self::AwaitingCommit => {
                *self = Self::Idle;
                false
            }
            Self::Idle => false,
        }
    }

    pub fn preedit(&self) -> Option<(&str, Option<(usize, usize)>)> {
        match self {
            Self::Composing { text, cursor } => Some((text, *cursor)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ImeState;

    #[test]
    fn only_commit_produces_text() {
        let mut ime = ImeState::default();
        ime.set_preedit("にほ".into(), Some((6, 6)));
        assert!(ime.blocks_keyboard_input());
        assert_eq!(ime.preedit().unwrap().0, "にほ");
        ime.set_preedit(String::new(), None);
        assert_eq!(ime.commit("日本".into()).as_deref(), Some("日本"));
        assert!(ime.preedit().is_none());
    }

    #[test]
    fn cancelled_preedit_releases_next_key() {
        let mut ime = ImeState::default();
        ime.set_preedit("x".into(), None);
        ime.set_preedit(String::new(), None);
        assert!(!ime.blocks_keyboard_input());
    }
}
