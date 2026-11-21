//! For the authors of configuration loaders and of module settings: the two ways a secret that
//! reaches a module through its configuration is kept out of the reports the graph writes.
//!
//! A loader deserialising configuration runs each deserialisation inside [`collect_secrets`] and
//! registers what it answers with [`CollectedSecrets::register`], so a `Secret<T>` read under
//! `#[serde(flatten)]` or `#[serde(untagged)]`, where no deserializer sees its name, is still
//! redacted (integrations DESIGN §2.3). A module or exporter whose `Module::settings` report a URL
//! writes it through [`setting_url`], so the comparison and the text of `DifferingSettings` carry
//! no credential (DESIGN §8.3).
//!
//! A panic's message is printed by the process's panic hook before ulo catches the panic, and the
//! default hook prints it raw: a registered secret or a connection string's password in the
//! message reaches stderr unredacted, whatever ulo reports of the panic afterwards. An application
//! calling [`install_redacting_hook`](crate::panic::install_redacting_hook) at the start of `main`
//! replaces the default hook with one that prints the message through the secrets the process's
//! apps registered; ulo never replaces the hook itself.
//!
//! Semver applies to this module as to the rest of the crate's public API: from 0.1.0, a change
//! that breaks a caller is released under a new breaking version (`0.x` to `0.(x+1)` before 1.0).
//! A pre-release of 0.1.0 may break it, and Cargo takes a later pre-release of the same version
//! for a requirement on an earlier one, so a crate depending on a pre-release pins it with `=`.

use crate::module::def::ModuleDef;
use crate::redact::Secret;

/// `url` as a module setting reports it: its scheme, host, port and path, never its userinfo,
/// query or fragment, and its scheme and `[unreadable]` when it does not read as a scheme and an
/// authority, or when the text kept could precede an `@` of `url`, which would make it part of a
/// userinfo (DESIGN §8.3). Every integration crate whose `settings` report a URL writes it
/// through this, so two settings differing only in a credential compare equal and neither is
/// printed.
///
/// ```
/// use ulo::secrets::setting_url;
///
/// assert_eq!(setting_url("redis://user:pw@cache:6379/2?pass=x#frag"), "redis://cache:6379/2");
/// assert_eq!(setting_url("redis://token/x@cache"), "redis://[unreadable]");
/// assert_eq!(setting_url("amqp://u:12?34@h/v"), "amqp://[unreadable]");
/// ```
pub fn setting_url(url: &str) -> String {
    crate::redact::setting_url(url)
}

/// A comma-separated list of URLs, as a client that splits it at its commas reads it, reported as
/// [`setting_url`] reports each element, joined with `,`. A comma may sit inside a password or a
/// query value, so until an element opens with a scheme or `//`, an element is `[unreadable]` when
/// a later one holds an `@` or an earlier one a `?` or `#`: what it keeps could be part of either.
///
/// ```
/// use ulo::secrets::setting_url_list;
///
/// assert_eq!(setting_url_list("nats://u:p@a:4222,nats://u:p@b:4222"), "nats://a:4222,nats://b:4222");
/// assert_eq!(setting_url_list("nats://u:12,34@h:4222"), "nats://[unreadable],h:4222");
/// assert_eq!(setting_url_list("nats://h:4222?token=a,b"), "nats://h:4222,[unreadable]");
/// ```
pub fn setting_url_list(list: &str) -> String {
    crate::redact::setting_url_list(list)
}

/// Runs one synchronous deserialisation with the secret collector open on this thread, answering
/// its value and every secret it produced (integrations §2.3, P6). `ulo-config` installs it around
/// each deserialisation it runs; `Secret<T>: Deserialize`, behind the `serde` feature, writes into
/// it, under `#[serde(flatten)]` and `#[serde(untagged)]` too. A collection open around this one
/// receives nothing of it. Without the `serde` feature nothing writes, and the answer is empty.
/// What it collects is also recorded for the hook
/// [`install_redacting_hook`](crate::panic::install_redacting_hook) installs, so a panic between
/// the load and the app's `wire` prints redacted under it.
pub fn collect_secrets<R>(f: impl FnOnce() -> R) -> (R, CollectedSecrets) {
    let (answer, texts) = crate::redact::collecting(f);
    crate::redact::record_for_process(&texts);
    (answer, CollectedSecrets { texts })
}

/// The secrets one [`collect_secrets`] call recorded: the text of each scalar a `Secret`'s inner
/// value was read from, whatever its type, so a `Secret<T>` whose `T` is not `Display` is
/// recorded by the text it was read from. A value read more than once is recorded once per read;
/// the registry skips a repeat. `Debug` writes the count alone.
#[derive(Clone, Default)]
pub struct CollectedSecrets {
    texts: Vec<String>,
}

impl CollectedSecrets {
    /// Registers each secret with the module through `m.secret(..)`, so the graph's redaction
    /// replaces it in every error the graph reports.
    pub fn register(self, m: &mut ModuleDef<'_>) {
        for text in self.texts {
            m.secret(&Secret::new(text));
        }
    }

    /// Appends `other`'s secrets, for a caller that runs several deserialisations and registers
    /// them together.
    pub fn append(&mut self, mut other: CollectedSecrets) {
        self.texts.append(&mut other.texts);
    }

    /// The recorded texts, for a caller that keeps them beside other secrets it registers later.
    pub fn texts(&self) -> &[String] {
        &self.texts
    }

    pub fn len(&self) -> usize {
        self.texts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.texts.is_empty()
    }
}

impl std::fmt::Debug for CollectedSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CollectedSecrets({} recorded)", self.texts.len())
    }
}
