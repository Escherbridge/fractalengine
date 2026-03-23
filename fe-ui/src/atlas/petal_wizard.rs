use fe_database::atlas::Visibility; // CROSS-CRATE: verify this type exists after merge

/// Step in the petal creation wizard.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum WizardStep {
    #[default]
    Identity,
    Metadata,
    Confirm,
}

/// Error type for tag validation.
#[derive(Debug, Clone, PartialEq)]
pub enum TagError {
    Duplicate,
    TooLong,
    LimitReached,
}

impl std::fmt::Display for TagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TagError::Duplicate => write!(f, "Tag already exists"),
            TagError::TooLong => write!(f, "Tag must be 32 characters or fewer"),
            TagError::LimitReached => write!(f, "Maximum 20 tags allowed"),
        }
    }
}

/// State resource for the petal creation wizard.
#[derive(Debug, Clone, Default)]
pub struct PetalWizardState {
    pub step: WizardStep,
    pub name: String,
    pub description: String,
    pub visibility: Visibility,
    pub tags: Vec<String>,
    /// Transient input buffer for the tag text field.
    pub tag_input: String,
    /// Transient error message to show in the UI.
    pub tag_error: Option<String>,
}

impl PetalWizardState {
    /// Returns `true` if the current step can be advanced.
    pub fn can_advance(&self) -> bool {
        match self.step {
            WizardStep::Identity => !self.name.is_empty() && self.name.len() <= 64,
            WizardStep::Metadata => true,
            WizardStep::Confirm => true,
        }
    }

    /// Advance to the next wizard step (no-op if already at Confirm).
    pub fn advance(&mut self) {
        if !self.can_advance() {
            return;
        }
        self.step = match self.step {
            WizardStep::Identity => WizardStep::Metadata,
            WizardStep::Metadata => WizardStep::Confirm,
            WizardStep::Confirm => WizardStep::Confirm,
        };
    }

    /// Reset the wizard to its initial state.
    pub fn reset(&mut self) {
        *self = PetalWizardState::default();
    }

    /// Validate and add a tag to the list.
    pub fn add_tag(&mut self, tag: String) -> Result<(), TagError> {
        if self.tags.len() >= 20 {
            return Err(TagError::LimitReached);
        }
        if tag.len() > 32 {
            return Err(TagError::TooLong);
        }
        let lower = tag.to_lowercase();
        if self.tags.iter().any(|t| t.to_lowercase() == lower) {
            return Err(TagError::Duplicate);
        }
        self.tags.push(tag);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_starts_at_step_one() {
        let state = PetalWizardState::default();
        assert_eq!(state.step, WizardStep::Identity);
    }

    #[test]
    fn cannot_advance_with_empty_name() {
        let mut state = PetalWizardState::default();
        state.name = String::new();
        assert!(!state.can_advance());
    }

    #[test]
    fn cannot_advance_with_name_over_64_chars() {
        let mut state = PetalWizardState::default();
        state.name = "x".repeat(65);
        assert!(!state.can_advance());
    }

    #[test]
    fn can_advance_with_valid_name_at_step_one() {
        let mut state = PetalWizardState::default();
        state.name = "My Petal".into();
        assert!(state.can_advance());
    }

    #[test]
    fn reset_returns_to_step_one_with_cleared_fields() {
        let mut state = PetalWizardState::default();
        state.name = "Temp".into();
        state.step = WizardStep::Confirm;
        state.reset();
        assert_eq!(state.step, WizardStep::Identity);
        assert!(state.name.is_empty());
    }

    #[test]
    fn add_tag_rejects_duplicate_case_insensitive() {
        let mut state = PetalWizardState::default();
        state.name = "X".into();
        state.add_tag("Foo".into()).unwrap();
        let result = state.add_tag("foo".into());
        assert!(result.is_err());
        assert_eq!(state.tags.len(), 1);
    }

    #[test]
    fn add_tag_rejects_over_32_chars() {
        let mut state = PetalWizardState::default();
        state.name = "X".into();
        let long_tag = "a".repeat(33);
        let result = state.add_tag(long_tag);
        assert!(result.is_err());
    }

    #[test]
    fn add_tag_rejects_when_count_is_20() {
        let mut state = PetalWizardState::default();
        state.name = "X".into();
        for i in 0..20 {
            state.add_tag(format!("tag{}", i)).unwrap();
        }
        let result = state.add_tag("one_more".into());
        assert!(result.is_err());
    }
}
