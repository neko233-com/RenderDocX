//! "Suggest remote replay" dialog state, mirroring upstream
//! `SuggestRemoteDialog` choices and enabling rules.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SuggestRemoteChoice {
    Local,
    Remote,
    Cancel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SuggestRemote {
    pub driver: String,
    pub machine_ident: String,
    pub remotes_available: bool,
    pub always_replay_locally: bool,
    pub choice: Option<SuggestRemoteChoice>,
}

impl SuggestRemote {
    pub fn new(
        driver: String,
        machine_ident: String,
        remotes_available: bool,
        always_replay_locally: bool,
    ) -> Self {
        Self {
            driver,
            machine_ident,
            remotes_available,
            always_replay_locally,
            choice: None,
        }
    }

    /// Upstream disables the remote action when no remote servers are
    /// configured or when the user chose to always replay locally.
    pub fn remote_enabled(&self) -> bool {
        self.remotes_available && !self.always_replay_locally
    }

    pub fn choose(&mut self, choice: SuggestRemoteChoice) {
        if choice == SuggestRemoteChoice::Remote && !self.remote_enabled() {
            return;
        }
        self.choice = Some(choice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_choice_requires_a_configured_remote() {
        let mut dialog = SuggestRemote::new("D3D11".into(), "machine".into(), false, false);
        assert!(!dialog.remote_enabled());
        dialog.choose(SuggestRemoteChoice::Remote);
        assert_eq!(dialog.choice, None);
        dialog.choose(SuggestRemoteChoice::Local);
        assert_eq!(dialog.choice, Some(SuggestRemoteChoice::Local));

        let mut dialog = SuggestRemote::new("D3D11".into(), "machine".into(), true, false);
        assert!(dialog.remote_enabled());
        dialog.choose(SuggestRemoteChoice::Remote);
        assert_eq!(dialog.choice, Some(SuggestRemoteChoice::Remote));

        let mut dialog = SuggestRemote::new("D3D11".into(), "machine".into(), true, true);
        assert!(!dialog.remote_enabled());
        dialog.choose(SuggestRemoteChoice::Cancel);
        assert_eq!(dialog.choice, Some(SuggestRemoteChoice::Cancel));
    }
}
