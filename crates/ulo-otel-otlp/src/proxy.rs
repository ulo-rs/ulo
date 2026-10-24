use std::fmt;

/// Where the client's proxy comes from. `Debug` never prints the URL, which may carry credentials.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) enum ProxyChoice {
    /// The client's default: `ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY` and `NO_PROXY`.
    #[default]
    Environment,
    /// This URL, whatever the environment says.
    Explicit(String),
    /// No proxy, whatever the environment says.
    Off,
}

impl fmt::Debug for ProxyChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ProxyChoice::Environment => "Environment",
            ProxyChoice::Explicit(_) => "Explicit(..)",
            ProxyChoice::Off => "Off",
        })
    }
}
