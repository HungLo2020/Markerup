use crate::workspace::EntryId;

/// Note-to-note navigation state, independent of the UI and storage backend.
#[derive(Debug, Default)]
pub struct NavigationState {
    back: Vec<EntryId>,
    forward: Vec<EntryId>,
}

impl NavigationState {
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn visit(&mut self, current: Option<&str>) {
        if let Some(current) = current {
            self.back.push(current.to_string());
            self.forward.clear();
        }
    }

    pub fn go_back(&mut self, current: Option<&str>) -> Option<EntryId> {
        let target = self.back.pop()?;
        if let Some(current) = current {
            self.forward.push(current.to_string());
        }
        Some(target)
    }

    pub fn go_forward(&mut self, current: Option<&str>) -> Option<EntryId> {
        let target = self.forward.pop()?;
        if let Some(current) = current {
            self.back.push(current.to_string());
        }
        Some(target)
    }

    pub fn rebase(&mut self, old: &str, new: &str) {
        self.back = self.back.iter().map(|id| rebase_id(id, old, new)).collect();
        self.forward = self
            .forward
            .iter()
            .map(|id| rebase_id(id, old, new))
            .collect();
    }

    pub fn remove(&mut self, id: &str) {
        self.back
            .retain(|value| value != id && !value.starts_with(&format!("{id}/")));
        self.forward
            .retain(|value| value != id && !value.starts_with(&format!("{id}/")));
    }
}

fn rebase_id(id: &str, old: &str, new: &str) -> EntryId {
    if id == old {
        return new.to_string();
    }
    id.strip_prefix(&format!("{old}/"))
        .map(|suffix| format!("{new}/{suffix}"))
        .unwrap_or_else(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::NavigationState;

    #[test]
    fn tracks_back_and_forward_navigation() {
        let mut navigation = NavigationState::default();
        navigation.visit(Some("one.md"));
        navigation.visit(Some("two.md"));

        assert_eq!(navigation.go_back(Some("three.md")), Some("two.md".into()));
        assert_eq!(navigation.go_back(Some("two.md")), Some("one.md".into()));
        assert_eq!(navigation.go_forward(Some("one.md")), Some("two.md".into()));
        assert!(navigation.can_go_forward());
    }

    #[test]
    fn rebases_and_removes_nested_paths() {
        let mut navigation = NavigationState::default();
        navigation.visit(Some("folder/note.md"));
        navigation.visit(Some("folder/other.md"));
        navigation.rebase("folder", "renamed");

        assert_eq!(
            navigation.go_back(Some("renamed/current.md")),
            Some("renamed/other.md".into())
        );
        navigation.remove("renamed");
        assert!(!navigation.can_go_back());
    }
}
