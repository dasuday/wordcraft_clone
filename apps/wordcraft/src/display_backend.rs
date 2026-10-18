//! Linux and the BSDs: whether the window runs on native Wayland or on X11 (XWayland in a Wayland
//! session).
//!
//! winit 0.30 has file drag-and-drop only on X11 (#220), so a Wayland session that also has an X
//! server runs through XWayland by default. Fcitx is the exception (#2950): through XWayland it
//! talks XIM, which garbled Vietnamese Telex (#2932) and once stopped typing altogether (#1169).
//! On native Wayland it reaches the window through the compositor's text-input protocol, when the
//! compositor connects it. IBus and other input methods stay on XIM: GNOME sets up IBus by
//! default, and no trouble with IBus over XIM has been reported. `WORDCRAFT_WAYLAND` overrides
//! both: `1` for native Wayland, `0` for XWayland.
//!
//! The decision reads the environment through a function, so the tests run on every platform.

use std::ffi::OsString;
use std::fmt;

/// The windowing backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// winit's own choice, which is native Wayland whenever a Wayland display is set.
    Wayland,
    /// X11, through XWayland in a Wayland session.
    X11,
}

/// Why a backend was chosen, for the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// `DISPLAY` is unset or empty, so there is no X server to use.
    NoX11Display,
    /// `WAYLAND_DISPLAY` and `WAYLAND_SOCKET` are unset or empty: an X11 session.
    NoWayland,
    /// `WORDCRAFT_WAYLAND` asks for native Wayland; its value.
    AskedForWayland(String),
    /// `WORDCRAFT_WAYLAND` asks for XWayland (`0`, `false`, `no` or `off`); its value.
    AskedForX11(String),
    /// Fcitx is set up: the variable that says so and its value.
    InputMethod(&'static str, String),
    /// A Wayland session without Fcitx: XWayland, for file drag-and-drop.
    DragAndDrop,
}

/// The backend and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub backend: Backend,
    pub reason: Reason,
}

/// The variables that can name Fcitx. `XMODIFIERS` comes first: it is the one winit's XIM client
/// reads.
const IM_VARS: [&str; 5] = ["XMODIFIERS", "GTK_IM_MODULE", "QT_IM_MODULE", "QT_IM_MODULES", "SDL_IM_MODULE"];

/// The input method names that pick native Wayland.
const FCITX: [&str; 2] = ["fcitx", "fcitx5"];

/// `WORDCRAFT_WAYLAND` values that ask for XWayland.
const OFF: [&str; 4] = ["0", "false", "no", "off"];

/// The longest value written to the log, in characters.
const LOGGED_CHARS: usize = 64;

/// Chooses the backend from the environment that `env` reads (`std::env::var_os` in the app).
pub fn choose(env: impl Fn(&'static str) -> Option<OsString>) -> Choice {
    let pick = |backend, reason| Choice { backend, reason };
    // As winit reads the display variables: empty or not UTF-8 counts as unset.
    let set = |name: &'static str| text(&env, name).is_some();
    if !set("DISPLAY") {
        return pick(Backend::Wayland, Reason::NoX11Display);
    }
    if !set("WAYLAND_DISPLAY") && !set("WAYLAND_SOCKET") {
        return pick(Backend::X11, Reason::NoWayland);
    }
    if let Some(value) = env("WORDCRAFT_WAYLAND") {
        let value = value.to_string_lossy();
        let value = value.trim();
        if OFF.iter().any(|off| value.eq_ignore_ascii_case(off)) {
            return pick(Backend::X11, Reason::AskedForX11(short(value)));
        }
        // Any other value has meant native Wayland since #220; empty means automatic.
        if !value.is_empty() {
            return pick(Backend::Wayland, Reason::AskedForWayland(short(value)));
        }
    }
    for var in IM_VARS {
        if let Some(value) = text(&env, var)
            && names_fcitx(var, &value)
        {
            return pick(Backend::Wayland, Reason::InputMethod(var, short(&value)));
        }
    }
    pick(Backend::X11, Reason::DragAndDrop)
}

/// A variable's value, unless it is unset, empty or not UTF-8.
fn text(env: &impl Fn(&'static str) -> Option<OsString>, name: &'static str) -> Option<String> {
    env(name).and_then(|v| v.into_string().ok()).filter(|v| !v.is_empty())
}

/// Whether `value` of `var` names Fcitx. `XMODIFIERS` names an input method after its first
/// `@im=`, up to the next `@`, as Xlib reads it. The toolkit variables name a module, or a list
/// of them: `:`-separated for GTK, `;`-separated for `QT_IM_MODULES`.
fn names_fcitx(var: &str, value: &str) -> bool {
    let is_fcitx = |name: &str| FCITX.iter().any(|f| name.trim().eq_ignore_ascii_case(f));
    if var == "XMODIFIERS" {
        value.split('@').skip(1).find_map(|m| m.strip_prefix("im=")).is_some_and(is_fcitx)
    } else {
        value.split([':', ';']).any(is_fcitx)
    }
}

/// `value` cut to [`LOGGED_CHARS`] characters.
fn short(value: &str) -> String {
    value.chars().take(LOGGED_CHARS).collect()
}

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.reason {
            Reason::NoX11Display => write!(f, "winit's choice: DISPLAY is not set"),
            Reason::NoWayland => write!(f, "X11: not a Wayland session"),
            Reason::AskedForWayland(v) => write!(f, "native Wayland: WORDCRAFT_WAYLAND={v}"),
            Reason::AskedForX11(v) => write!(f, "XWayland: WORDCRAFT_WAYLAND={v}"),
            Reason::InputMethod(var, v) => {
                write!(f, "native Wayland: Fcitx is set up ({var}={v}); WORDCRAFT_WAYLAND=0 uses XWayland for file drag-and-drop")
            }
            Reason::DragAndDrop => write!(f, "XWayland, for file drag-and-drop: Fcitx is not set up; WORDCRAFT_WAYLAND=1 uses native Wayland"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Wayland session that also has XWayland, plus `vars`.
    fn wayland_session(vars: &[(&str, &str)]) -> Choice {
        let mut all = vec![("DISPLAY", ":0"), ("WAYLAND_DISPLAY", "wayland-0")];
        all.extend_from_slice(vars);
        choose(env_of(&all))
    }

    fn env_of(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let vars: Vec<(String, OsString)> = vars.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    }

    #[test]
    fn the_app_reads_the_process_environment() {
        // The same call as in `main`, so it compiles and runs on every platform.
        assert!(!choose(std::env::var_os).to_string().is_empty());
    }

    #[test]
    fn a_wayland_session_without_fcitx_keeps_xwayland_for_drag_and_drop() {
        let choice = wayland_session(&[]);
        assert_eq!(choice, Choice { backend: Backend::X11, reason: Reason::DragAndDrop });
        assert!(choice.to_string().contains("WORDCRAFT_WAYLAND=1"));
    }

    #[test]
    fn fcitx_in_a_wayland_session_picks_native_wayland() {
        for (var, value) in [
            ("XMODIFIERS", "@im=fcitx"),
            ("XMODIFIERS", "@im=fcitx5"),
            ("XMODIFIERS", "@im=FCITX"),
            ("GTK_IM_MODULE", "fcitx"),
            ("QT_IM_MODULE", "fcitx"),
            ("QT_IM_MODULES", "wayland;fcitx;ibus"),
            ("SDL_IM_MODULE", "fcitx5"),
        ] {
            let choice = wayland_session(&[(var, value)]);
            assert_eq!(choice, Choice { backend: Backend::Wayland, reason: Reason::InputMethod(var, value.into()) }, "{var}={value}");
            let logged = choice.to_string();
            assert!(logged.contains(&format!("{var}={value}")) && logged.contains("WORDCRAFT_WAYLAND=0"), "{logged}");
        }
    }

    #[test]
    fn xmodifiers_is_named_first_when_several_variables_are_set() {
        let choice = wayland_session(&[("QT_IM_MODULE", "fcitx"), ("XMODIFIERS", "@im=fcitx"), ("SDL_IM_MODULE", "fcitx")]);
        assert_eq!(choice.reason, Reason::InputMethod("XMODIFIERS", "@im=fcitx".into()));
        // XMODIFIERS naming no server leaves the toolkit variables to decide.
        let choice = wayland_session(&[("XMODIFIERS", "@im=none"), ("QT_IM_MODULE", "fcitx")]);
        assert_eq!(choice.reason, Reason::InputMethod("QT_IM_MODULE", "fcitx".into()));
    }

    #[test]
    fn stock_gnome_with_ibus_keeps_xwayland() {
        let choice = wayland_session(&[("XMODIFIERS", "@im=ibus"), ("QT_IM_MODULE", "ibus"), ("GTK_IM_MODULE", "ibus")]);
        assert_eq!(choice, Choice { backend: Backend::X11, reason: Reason::DragAndDrop });
    }

    #[test]
    fn other_input_methods_and_junk_keep_xwayland() {
        for (var, value) in [
            ("XMODIFIERS", "@im=ibus"),
            ("XMODIFIERS", "@im=kinput2"),
            ("XMODIFIERS", "@im=fcitxx"),
            ("XMODIFIERS", "im=fcitx"),
            ("XMODIFIERS", "@im=none@im=fcitx"),
            ("XMODIFIERS", "@im=none"),
            ("XMODIFIERS", "@im=NONE"),
            ("XMODIFIERS", "@im=local"),
            ("XMODIFIERS", "@im="),
            ("XMODIFIERS", "@im=  "),
            ("XMODIFIERS", "fcitx"),
            ("XMODIFIERS", "@other=fcitx"),
            ("XMODIFIERS", ""),
            ("XMODIFIERS", "   "),
            ("XMODIFIERS", "@@@"),
            ("GTK_IM_MODULE", "ibus"),
            ("GTK_IM_MODULE", "wayland"),
            ("GTK_IM_MODULE", "xim"),
            ("GTK_IM_MODULE", "gtk-im-context-simple"),
            ("GTK_IM_MODULE", "simple"),
            ("GTK_IM_MODULE", ":::"),
            ("QT_IM_MODULE", "ibus"),
            ("QT_IM_MODULES", "wayland;ibus"),
            ("QT_IM_MODULE", "compose"),
            ("QT_IM_MODULE", "none"),
            ("SDL_IM_MODULE", ""),
        ] {
            assert_eq!(wayland_session(&[(var, value)]).reason, Reason::DragAndDrop, "{var}={value:?}");
        }
    }

    #[test]
    fn modifier_lists_and_module_lists_are_read() {
        assert_eq!(wayland_session(&[("XMODIFIERS", "@foo=bar@im=fcitx")]).backend, Backend::Wayland);
        assert_eq!(wayland_session(&[("XMODIFIERS", "@foo=bar @im=fcitx ")]).backend, Backend::Wayland);
        assert_eq!(wayland_session(&[("GTK_IM_MODULE", "simple:fcitx")]).backend, Backend::Wayland);
        assert_eq!(wayland_session(&[("QT_IM_MODULES", " wayland ; fcitx5 ")]).backend, Backend::Wayland);
        assert_eq!(wayland_session(&[("GTK_IM_MODULE", "xim:simple:ibus")]).backend, Backend::X11);
    }

    #[test]
    fn wordcraft_wayland_overrides_the_input_method_rule() {
        for value in ["1", "true", "yes", "on", "anything"] {
            let choice = wayland_session(&[("WORDCRAFT_WAYLAND", value)]);
            assert_eq!(choice, Choice { backend: Backend::Wayland, reason: Reason::AskedForWayland(value.into()) });
        }
        for value in ["0", "false", "No", " OFF "] {
            let choice = wayland_session(&[("WORDCRAFT_WAYLAND", value), ("XMODIFIERS", "@im=fcitx")]);
            assert_eq!(choice, Choice { backend: Backend::X11, reason: Reason::AskedForX11(value.trim().into()) });
            assert!(choice.to_string().starts_with("XWayland"));
        }
        // Empty means automatic.
        assert_eq!(wayland_session(&[("WORDCRAFT_WAYLAND", ""), ("XMODIFIERS", "@im=fcitx")]).backend, Backend::Wayland);
        assert_eq!(wayland_session(&[("WORDCRAFT_WAYLAND", " ")]).reason, Reason::DragAndDrop);
    }

    #[test]
    fn without_an_x_server_the_window_stays_on_wayland() {
        for vars in
            [&[("WAYLAND_DISPLAY", "wayland-0"), ("WORDCRAFT_WAYLAND", "0")][..], &[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", "")][..], &[][..]]
        {
            assert_eq!(choose(env_of(vars)), Choice { backend: Backend::Wayland, reason: Reason::NoX11Display }, "{vars:?}");
        }
    }

    #[test]
    fn an_x11_session_stays_on_x11_whatever_is_asked() {
        for vars in [
            &[("DISPLAY", ":0")][..],
            &[("DISPLAY", ":0"), ("WORDCRAFT_WAYLAND", "1")][..],
            &[("DISPLAY", ":0"), ("XMODIFIERS", "@im=fcitx")][..],
            &[("DISPLAY", ":0"), ("WAYLAND_DISPLAY", "")][..],
        ] {
            assert_eq!(choose(env_of(vars)), Choice { backend: Backend::X11, reason: Reason::NoWayland }, "{vars:?}");
        }
        // winit also takes a Wayland socket handed over by the parent.
        assert_eq!(choose(env_of(&[("DISPLAY", ":0"), ("WAYLAND_SOCKET", "5")])).reason, Reason::DragAndDrop);
    }

    #[test]
    fn long_values_are_cut_in_the_log() {
        let long = format!("fcitx:{}", "é".repeat(500));
        let choice = wayland_session(&[("GTK_IM_MODULE", &long)]);
        let Reason::InputMethod(_, logged) = &choice.reason else { panic!("{choice:?}") };
        assert_eq!(logged.chars().count(), LOGGED_CHARS);
        let choice = wayland_session(&[("WORDCRAFT_WAYLAND", &"y".repeat(500))]);
        assert_eq!(choice.reason, Reason::AskedForWayland("y".repeat(LOGGED_CHARS)));
    }

    #[cfg(unix)]
    #[test]
    fn values_that_are_not_utf8_never_panic() {
        use std::os::unix::ffi::OsStringExt;
        let bad = |bytes: &[u8]| OsString::from_vec(bytes.to_vec());
        let env = |vars: Vec<(&'static str, OsString)>| move |name: &str| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone());
        let session = |extra: Vec<(&'static str, OsString)>| {
            let mut vars = vec![("DISPLAY", OsString::from(":0")), ("WAYLAND_DISPLAY", OsString::from("wayland-0"))];
            vars.extend(extra);
            choose(env(vars))
        };
        // An input method variable that isn't UTF-8 doesn't name Fcitx.
        for var in IM_VARS {
            assert_eq!(session(vec![(var, bad(b"@im=fcitx:\xff\xfe"))]).reason, Reason::DragAndDrop, "{var}");
        }
        // WORDCRAFT_WAYLAND keeps its old meaning: any value but an "off" one is native Wayland.
        let choice = session(vec![("WORDCRAFT_WAYLAND", bad(b"\xff"))]);
        assert_eq!(choice.backend, Backend::Wayland);
        assert!(!choice.to_string().is_empty());
        // Display variables that aren't UTF-8 count as unset, as winit reads them.
        assert_eq!(choose(env(vec![("DISPLAY", bad(b":\xff")), ("WAYLAND_DISPLAY", OsString::from("w"))])).reason, Reason::NoX11Display);
        assert_eq!(choose(env(vec![("DISPLAY", OsString::from(":0")), ("WAYLAND_DISPLAY", bad(b"\xff"))])).reason, Reason::NoWayland);
    }
}
