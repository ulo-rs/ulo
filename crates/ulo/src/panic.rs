//! The process's panic hook. Rust runs the installed hook for every panic, on the panicking thread,
//! before the unwinding reaches any `catch_unwind`, so what ulo redacts in a caught panic reaches
//! its reports and never what the hook already printed; the default hook prints the raw message.
//! [`install_redacting_hook`] replaces it with one that prints the message redacted. ulo never
//! installs it: the hook is the process's, and a library replacing it would replace the
//! application's own.

use std::any::Any;
use std::backtrace::Backtrace;
use std::io::Write;
use std::panic::{Location, PanicHookInfo};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::redact::{self, SecretRegistry};

/// Replaces the process's panic hook with one that prints each panic's message after the redaction
/// ulo applies to a panic it catches: every secret the process's apps registered replaced by
/// `[redacted]`, and the userinfo of anything shaped like a URL stripped. For an application to
/// call at the start of `main`, before anything can panic.
///
/// The secrets are those of every graph an app took in this process, at `wire` and at each `load`,
/// an app since dropped included, and every text [`collect_secrets`] collected, so the hook can be
/// installed before the app is wired. A panic before anything is recorded, such as one while a
/// configuration loads outside `collect_secrets`, is printed with the userinfo strip alone.
///
/// The hook writes to stderr `panicked at <file>:<line>:<column>:` and the redacted message, then
/// the backtrace when `RUST_BACKTRACE` enables one, every frame under `full`, or, after the first
/// panic only, the note the default hook writes naming `RUST_BACKTRACE`. `RUST_LIB_BACKTRACE`
/// leaves it unchanged, as it leaves the default hook. The thread's name is left out: reading it panics
/// once the thread's local data is gone, and a panic inside a panic hook aborts the process. For
/// the same reason nothing in the hook unwraps, a failed write to stderr is ignored, and the
/// secrets are read without waiting on their lock: a read that finds a writer in it on every one
/// of a bounded number of tries prints
/// `[message withheld: the secrets to redact it with could not be read]` in place of the message.
///
/// The hook it replaces is not called, whether the default, the application's own or a library's.
/// A hook reads the message from the `PanicHookInfo` it is given, which nothing outside the
/// standard library builds with another message, so calling the previous hook would print the
/// raw message beside the redacted one. An application with a hook of its own, a crash reporter
/// say, installs that hook instead of this one, and ulo redacts nothing it prints. Calling this
/// again installs the same hook again.
///
/// # Panics
///
/// When called from a panicking thread, as [`std::panic::set_hook`] does.
///
/// ```
/// ulo::panic::install_redacting_hook();
/// // Wire, connect and serve the app: a panic anywhere in the process now prints redacted.
/// let caught = std::panic::catch_unwind(|| panic!("connecting to postgres://app:hunter2@db/main"));
/// assert!(caught.is_err());
/// // stderr: panicked at <file>:<line>:<column>:
/// //         connecting to postgres://[redacted]@db/main
/// ```
///
/// [`collect_secrets`]: crate::secrets::collect_secrets
pub fn install_redacting_hook() {
    std::panic::set_hook(Box::new(hook));
}

fn hook(info: &PanicHookInfo<'_>) {
    let report = redact::with_process_secrets(|secrets| report(info.location(), info.payload(), secrets))
        .unwrap_or_else(|| withheld(info.location()));
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(report.as_bytes());
    match backtrace_style() {
        SHORT => {
            let _ = writeln!(stderr, "stack backtrace:\n{}", Backtrace::force_capture());
        }
        FULL => {
            let _ = writeln!(stderr, "stack backtrace:\n{:#}", Backtrace::force_capture());
        }
        _ => {
            if FIRST_PANIC.swap(false, Ordering::Relaxed) {
                let _ = writeln!(stderr, "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace");
            }
        }
    }
    let _ = stderr.flush();
}

/// Cleared by the first panic the hook prints, which alone carries the backtrace note.
static FIRST_PANIC: AtomicBool = AtomicBool::new(true);

const UNREAD: u8 = 0;
const OFF: u8 = 1;
const SHORT: u8 = 2;
const FULL: u8 = 3;

/// `RUST_BACKTRACE` as the default hook reads it, read once: unset or `0` prints no backtrace,
/// `full` every frame, any other value the short form. `RUST_LIB_BACKTRACE` is not read:
/// `Backtrace::capture` reads it ahead of `RUST_BACKTRACE`, and it configures the backtraces
/// errors capture, not a panic's.
fn backtrace_style() -> u8 {
    static STYLE: AtomicU8 = AtomicU8::new(UNREAD);
    let style = STYLE.load(Ordering::Relaxed);
    if style != UNREAD {
        return style;
    }
    let style = match std::env::var_os("RUST_BACKTRACE") {
        None => OFF,
        Some(value) if value == "0" => OFF,
        Some(value) if value == "full" => FULL,
        Some(_) => SHORT,
    };
    STYLE.store(style, Ordering::Relaxed);
    style
}

/// What the hook prints for a panic at `location` carrying `payload`, its message redacted with
/// `secrets` by the function a caught panic's message passes through.
fn report(location: Option<&Location<'_>>, payload: &(dyn Any + Send), secrets: &SecretRegistry) -> String {
    let message = redact::scrub(secrets, &redact::panic_text(payload));
    format!("{}:\n{message}\n", heading(location))
}

/// What the hook prints when the secrets could not be read.
fn withheld(location: Option<&Location<'_>>) -> String {
    format!("{}:\n[message withheld: the secrets to redact it with could not be read]\n", heading(location))
}

fn heading(location: Option<&Location<'_>>) -> String {
    match location {
        Some(at) => format!("panicked at {at}"),
        None => "panicked".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "report-token-4471";

    fn registry() -> SecretRegistry {
        let mut secrets = SecretRegistry::default();
        secrets.register(SECRET.to_owned());
        secrets
    }

    /// The report replaces a registered secret and a url's userinfo, keeps the rest of the message
    /// and names the location.
    #[test]
    fn a_report_carries_the_message_redacted_at_its_location() {
        let payload: Box<dyn Any + Send> = Box::new(format!("connecting with {SECRET} to postgres://app:pw@db/main"));
        let report = report(Some(Location::caller()), &*payload, &registry());
        assert!(!report.contains(SECRET) && !report.contains("app:pw"), "{report}");
        assert!(report.contains("connecting with [redacted] to postgres://[redacted]@db/main"), "{report}");
        assert!(report.starts_with(&format!("panicked at {}:", file!())), "{report}");
    }

    /// A payload that is no string is reported by the text a caught panic's carries.
    #[test]
    fn a_payload_that_is_no_string_is_named_as_such() {
        let payload: Box<dyn Any + Send> = Box::new(7u32);
        let report = report(None, &*payload, &registry());
        assert_eq!(report, "panicked:\na panic whose payload is not a string\n");
    }

    /// A writer holding the registry's lock on the panicking thread, which the swap never does,
    /// leaves the hook's read refused rather than waiting: the message is withheld, and the read
    /// returns.
    #[test]
    fn a_registry_whose_lock_is_held_is_withheld_and_never_waited_on() {
        redact::record_for_process(&[SECRET.to_owned()]);
        let held = redact::hold_process_lock_for_test();
        let read = redact::with_process_secrets(|secrets| secrets.texts.len());
        drop(held);
        assert_eq!(read, None, "the read did not see the lock held");
        let after = redact::with_process_secrets(|secrets| secrets.texts.contains(&SECRET.to_owned()));
        assert_eq!(after, Some(true), "the recorded secret, once the lock is released");
    }
}
