use ulo_health::Report;

/// The media type of the body, from the IETF health-check response draft, which expired in April
/// 2022 and is followed as a shape and not as a contract.
pub const MEDIA_TYPE: &str = "application/health+json";

/// The body for `report`: `status` is `pass` or `fail`, and `checks` is present only with
/// `detail`, each check with its redacted reason.
pub(crate) fn render(report: &Report, detail: bool) -> String {
    let _ = (report, detail);
    todo!()
}
