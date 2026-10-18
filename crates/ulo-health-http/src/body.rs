use std::error::Error;

use serde_json::{Map, Value};
use ulo::{AppHandle, BoxError};
use ulo_health::{Health, Report};

/// The media type of the body, from the IETF health-check response draft, which expired in April
/// 2022 and is followed as a shape and not as a contract.
pub const MEDIA_TYPE: &str = "application/health+json";

/// How far a down reason's `source()` chain is followed, as the core's redaction follows one.
const MAX_CHAIN: usize = 32;

/// The body for `report`: `status` is `pass` or `fail`, and `checks` is present only with
/// `detail`, each check with its redacted reason.
///
/// `checks` maps a check's name to an array, as the draft's `checks` object does, so two checks of
/// one name, two keyed stores of one kind, are both listed.
pub(crate) fn render(report: &Report, detail: bool, app: &AppHandle) -> String {
    let mut body = Map::new();
    body.insert(String::from("status"), Value::from(status(report.is_up())));
    if detail {
        let mut checks = Map::new();
        for result in report.results() {
            let mut entry = Map::new();
            entry.insert(String::from("status"), Value::from(status(result.health().is_up())));
            if let Health::Down(reason) = result.health() {
                entry.insert(String::from("output"), Value::from(redacted(reason, app)));
            }
            let slot = checks.entry(result.name().to_owned()).or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(list) = slot {
                list.push(Value::Object(entry));
            }
        }
        body.insert(String::from("checks"), Value::Object(checks));
    }
    Value::Object(body).to_string()
}

fn status(up: bool) -> &'static str {
    if up { "pass" } else { "fail" }
}

/// The reason through the app's redaction. The report is shared by every probe of its run, so the
/// reason is read rather than moved: its chain is flattened here, as the core flattens one, and the
/// text redacted.
fn redacted(reason: &BoxError, app: &AppHandle) -> String {
    let error: &(dyn Error + 'static) = &**reason;
    let mut text = error.to_string();
    let mut next = error.source();
    let mut links = 0;
    while let Some(cause) = next {
        if links == MAX_CHAIN {
            break;
        }
        let message = cause.to_string();
        if !message.is_empty() && !text.contains(&message) {
            text.push_str(": ");
            text.push_str(&message);
        }
        next = cause.source();
        links += 1;
    }
    app.redact(BoxError::from(text)).to_string()
}
