use ulo::{Bound, BoxError, BoxFuture};

/// A named question the app answers about one dependency.
///
/// A module contributes one with `m.contribute::<dyn Check>()`, and a keyed module's contribution
/// keeps its key, so two keyed databases contribute two checks. A check that does not answer within
/// its [`bound`](Self::bound) is down, never up, and no result is kept between runs.
pub trait Check: Send + Sync + 'static {
    /// The name the report and the `checks` object print.
    fn name(&self) -> &str;

    /// One evaluation, run by each probe that reads the check. The health module races it against
    /// the bound and reads a panic as down, so the future need not time itself.
    fn check(&self) -> BoxFuture<'_, Health>;

    /// `Bound::Default` is 2 s; `Bound::After(Duration::ZERO)` is refused at `connect`, where the
    /// health module first reads the collection; `Bound::Unbounded` is the opt-out.
    fn bound(&self) -> Bound {
        Bound::Default
    }

    /// Integrations never contribute to liveness; an application writes `Probe::Liveness` only for
    /// a check it declares itself.
    fn probe(&self) -> Probe {
        Probe::Readiness
    }
}

/// What a check answers.
#[derive(Debug)]
pub enum Health {
    Up,
    /// Down, with a reason. The reason is redacted before it is shown.
    Down(BoxError),
}

impl Health {
    pub fn down(reason: impl Into<BoxError>) -> Self {
        Health::Down(reason.into())
    }

    pub fn is_up(&self) -> bool {
        matches!(self, Health::Up)
    }
}

/// The probe a check belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Probe {
    Readiness,
    Liveness,
}
