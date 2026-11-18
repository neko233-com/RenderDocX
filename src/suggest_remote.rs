//! "Suggest remote replay" dialog state, mirroring upstream
//! `SuggestRemoteDialog` choices and enabling rules.

/// Upstream `MachineIdent_*` constants from `renderdoc/os/os_specific.h`.
pub const MACHINE_IDENT_OS_MASK: u64 = 0x0000_00ff;
const MACHINE_IDENT_WINDOWS: u64 = 0x1;
const MACHINE_IDENT_LINUX: u64 = 0x2;
const MACHINE_IDENT_MACOS: u64 = 0x4;
const MACHINE_IDENT_ANDROID: u64 = 0x8;
const MACHINE_IDENT_IOS: u64 = 0x10;
const MACHINE_IDENT_ARCH_X86: u64 = 0x0000_0100;
const MACHINE_IDENT_ARCH_ARM: u64 = 0x0000_0200;
const MACHINE_IDENT_32BIT: u64 = 0x0000_1000;
const MACHINE_IDENT_64BIT: u64 = 0x0000_2000;
const MACHINE_IDENT_GPU_MASK: u64 = 0x00ff_0000;

/// Native equivalent of upstream `OSUtility::GetMachineIdent()`.
pub fn local_machine_ident() -> u64 {
    let mut ident = match std::env::consts::OS {
        "windows" => MACHINE_IDENT_WINDOWS,
        "linux" => MACHINE_IDENT_LINUX,
        "macos" => MACHINE_IDENT_MACOS,
        "android" => MACHINE_IDENT_ANDROID,
        "ios" => MACHINE_IDENT_IOS,
        _ => 0,
    };
    if cfg!(any(target_arch = "arm", target_arch = "aarch64")) {
        ident |= MACHINE_IDENT_ARCH_ARM;
    } else {
        ident |= MACHINE_IDENT_ARCH_X86;
    }
    if cfg!(target_pointer_width = "64") {
        ident |= MACHINE_IDENT_64BIT;
    } else {
        ident |= MACHINE_IDENT_32BIT;
    }
    ident
}

/// Native equivalent of upstream `OSUtility::MakeMachineIdentString()`.
pub fn machine_ident_string(ident: u64) -> String {
    let mut text = String::new();
    for (bit, name) in [
        (MACHINE_IDENT_WINDOWS, "Windows "),
        (MACHINE_IDENT_LINUX, "Linux "),
        (MACHINE_IDENT_MACOS, "macOS "),
        (MACHINE_IDENT_ANDROID, "Android "),
        (MACHINE_IDENT_IOS, "iOS "),
    ] {
        if ident & bit != 0 {
            text.push_str(name);
            break;
        }
    }
    if ident & MACHINE_IDENT_ARCH_X86 != 0 {
        text.push_str("x86 ");
    } else if ident & MACHINE_IDENT_ARCH_ARM != 0 {
        text.push_str("ARM ");
    }
    if ident & MACHINE_IDENT_32BIT != 0 {
        text.push_str("32-bit ");
    } else if ident & MACHINE_IDENT_64BIT != 0 {
        text.push_str("64-bit ");
    }
    for (bit, name) in [
        (0x0001_0000, "ARM GPU "),
        (0x0002_0000, "AMD GPU "),
        (0x0004_0000, "Imagination GPU "),
        (0x0008_0000, "Intel GPU "),
        (0x0010_0000, "nVidia GPU "),
        (0x0020_0000, "QUALCOMM GPU "),
        (0x0040_0000, "Samsung GPU "),
        (0x0080_0000, "Verisilicon GPU "),
    ] {
        if ident & MACHINE_IDENT_GPU_MASK == bit {
            text.push_str(name);
            break;
        }
    }
    text.trim_end().to_owned()
}

/// Decide whether remote replay should be suggested. Upstream asks the replay
/// core for `ReplaySupport::SuggestRemote`, which at minimum fires when the
/// capture was made on a different operating system (machine-ident OS mask).
/// `always_replay_locally` suppresses the prompt.
pub fn should_suggest(
    capture_machine_ident: u64,
    local_machine_ident: u64,
    always_replay_locally: bool,
) -> bool {
    if always_replay_locally || capture_machine_ident == 0 {
        return false;
    }
    (capture_machine_ident & MACHINE_IDENT_OS_MASK)
        != (local_machine_ident & MACHINE_IDENT_OS_MASK)
}

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
    fn should_suggest_from_machine_ident() {
        let local = local_machine_ident();
        assert!(should_suggest(
            MACHINE_IDENT_ANDROID | MACHINE_IDENT_ARCH_ARM | MACHINE_IDENT_64BIT,
            local,
            false
        ));
        // Different arch/bitness on the same OS does not trigger upstream's
        // OS-mask check.
        assert!(!should_suggest(
            local ^ MACHINE_IDENT_ARCH_X86 ^ MACHINE_IDENT_ARCH_ARM,
            local,
            false
        ));
        assert!(!should_suggest(0, local, false)); // unknown capture machine
        assert!(!should_suggest(MACHINE_IDENT_LINUX, local, true)); // opted out
        assert!(local & MACHINE_IDENT_OS_MASK != 0);
        assert!((local & (MACHINE_IDENT_32BIT | MACHINE_IDENT_64BIT)) != 0);
        let text = machine_ident_string(local);
        assert!(text.contains("64-bit") || text.contains("32-bit"), "{text}");
        assert!(!machine_ident_string(0).contains("GPU"));
    }

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
