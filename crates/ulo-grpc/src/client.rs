//! Client modules (transports DESIGN §6.3): a tonic-generated client bound under its own type, its
//! channel connected lazily, so `connect` does no network I/O.
//!
//! ```ignore
//! imports = [GrpcClientModule::<pb::UserServiceClient<Channel>>::for_root(
//!     GrpcEndpoint::new("https://users:443").tls(ClientTls::system_roots()),
//! ).keyed::<UsersApi>()]
//! ```

use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::time::Duration;

use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};
use ulo::{BoxError, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity};

/// A tonic client built over a channel. `ulo-build` writes the impl for every client it
/// generates, `UserServiceClient::new(channel)`.
pub trait GrpcClient: Send + Sync + 'static {
    fn from_channel(channel: Channel) -> Self;
}

/// Binds client `C`, exported, usually keyed so two endpoints of one service coexist:
/// `Dep<pb::UserServiceClient<Channel>, UsersApi>`.
///
/// The URI is parsed when the app wires, a bad one reported with the wiring errors. The TLS roots
/// are loaded and the channel built when the app connects; the channel opens its connection on
/// the first call.
pub struct GrpcClientModule<C: GrpcClient> {
    pub(crate) endpoint: GrpcEndpoint,
    pub(crate) _client: PhantomData<fn() -> C>,
}

impl<C: GrpcClient> GrpcClientModule<C> {
    pub fn for_root(endpoint: GrpcEndpoint) -> Self {
        GrpcClientModule { endpoint, _client: PhantomData }
    }
}

impl<C: GrpcClient> Module for GrpcClientModule<C> {
    fn identity(&self) -> ModuleIdentity {
        let key = ClientKey::<C> { uri: self.endpoint.uri.clone(), tls: self.endpoint.tls.clone(), _client: PhantomData };
        ModuleIdentity::of_value(&key).label("GrpcClientModule")
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.try_value(ClientEndpoint::<C>::parse(&self.endpoint));
        m.try_singleton(|endpoint: Dep<ClientEndpoint<C>>| async move { endpoint.connect() });
        m.export::<C>();
    }
}

/// Where a client connects: the URI and its TLS. The URI is parsed when the app wires, so a bad
/// one is reported with the wiring errors. An `https` URI without `tls` trusts the platform's
/// roots, as `ClientTls::system_roots()` does.
#[derive(Clone, Debug)]
pub struct GrpcEndpoint {
    pub(crate) uri: String,
    pub(crate) tls: Option<ClientTls>,
}

impl GrpcEndpoint {
    pub fn new(uri: impl Into<String>) -> Self {
        GrpcEndpoint { uri: uri.into(), tls: None }
    }

    pub fn tls(mut self, tls: ClientTls) -> Self {
        self.tls = Some(tls);
        self
    }
}

/// The roots a client trusts.
#[derive(Clone, Debug)]
pub struct ClientTls {
    pub(crate) roots: Roots,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Roots {
    System,
    Pem(Vec<u8>),
}

impl ClientTls {
    /// The platform's trust store.
    pub fn system_roots() -> Self {
        ClientTls { roots: Roots::System }
    }

    /// The CA certificates in `pem`, for a service whose certificate a private CA signed.
    pub fn ca_pem(pem: impl Into<Vec<u8>>) -> Self {
        ClientTls { roots: Roots::Pem(pem.into()) }
    }

    fn config(&self) -> ClientTlsConfig {
        match &self.roots {
            Roots::System => ClientTlsConfig::new().with_native_roots(),
            Roots::Pem(pem) => ClientTlsConfig::new().ca_certificate(Certificate::from_pem(pem)),
        }
    }
}

/// `request` with `grpc-timeout` set from `exec`'s remaining deadline. There is no ambient
/// execution, so nothing forwards a deadline on its own.
///
/// ```ignore
/// let user = users.get_user(ulo_grpc::outgoing(&exec, tonic::Request::new(query))?).await?;
/// ```
///
/// A deadline already passed fails here, DEADLINE_EXCEEDED, so the call is never made: sent with
/// the shortest `grpc-timeout`, it would reach a callee that can run its handler while the caller
/// is told it timed out. A deadline further off than `99999999H`, about 11,415 years and the
/// longest the grammar holds, is sent as that. The remaining time is read on tokio's clock, which
/// `ulo-runtime-tokio`'s `Timer` reads too.
pub fn outgoing<T>(exec: &ExecutionRef, request: tonic::Request<T>) -> Result<tonic::Request<T>, tonic::Status> {
    let Some(deadline) = exec.deadline() else { return Ok(request) };
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now().into_std());
    if remaining.is_zero() {
        return Err(tonic::Status::deadline_exceeded("the calling execution's deadline has passed"));
    }
    Ok(timed(request, remaining))
}

/// The longest `grpc-timeout`, `99999999H`.
const LONGEST: Duration = Duration::from_secs(99_999_999 * 3600);

/// `request` with `grpc-timeout` written for `remaining`. tonic writes the finest unit whose
/// amount fits eight digits, truncated, and panics for a duration longer than `99999999H`, so
/// `remaining` is held to that first.
fn timed<T>(mut request: tonic::Request<T>, remaining: Duration) -> tonic::Request<T> {
    request.set_timeout(remaining.clamp(Duration::from_nanos(1), LONGEST));
    request
}

/// The module's identity: the client type and the endpoint, so two endpoints of one client are two
/// modules and one endpoint imported twice is one.
struct ClientKey<C> {
    uri: String,
    tls: Option<ClientTls>,
    _client: PhantomData<fn() -> C>,
}

impl<C> Clone for ClientKey<C> {
    fn clone(&self) -> Self {
        ClientKey { uri: self.uri.clone(), tls: self.tls.clone(), _client: PhantomData }
    }
}

impl<C> PartialEq for ClientKey<C> {
    fn eq(&self, other: &Self) -> bool {
        self.uri == other.uri && self.tls.as_ref().map(|tls| &tls.roots) == other.tls.as_ref().map(|tls| &tls.roots)
    }
}

impl<C> Eq for ClientKey<C> {}

impl<C> Hash for ClientKey<C> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.uri.hash(state);
        self.tls.as_ref().map(|tls| &tls.roots).hash(state);
    }
}

/// The parsed endpoint, bound unexported beside the client it builds.
struct ClientEndpoint<C> {
    endpoint: Endpoint,
    tls: Option<ClientTls>,
    _client: PhantomData<fn() -> C>,
}

impl<C: GrpcClient> ClientEndpoint<C> {
    fn parse(endpoint: &GrpcEndpoint) -> Result<Self, BoxError> {
        let parsed = Endpoint::from_shared(endpoint.uri.clone())
            .map_err(|error| BoxError::from(format!("`{}` is not a gRPC endpoint URI: {error}", endpoint.uri)))?;
        let https = parsed.uri().scheme_str() == Some("https");
        let tls = endpoint.tls.clone().or_else(|| https.then(ClientTls::system_roots));
        Ok(ClientEndpoint { endpoint: parsed, tls, _client: PhantomData })
    }

    /// The client over a lazily connected channel. Loading the platform's roots reads the trust
    /// store, so it happens here, when the app connects, and not where the module registers.
    fn connect(&self) -> Result<C, BoxError> {
        let endpoint = match &self.tls {
            Some(tls) => self.endpoint.clone().tls_config(tls.config())?,
            None => self.endpoint.clone(),
        };
        Ok(C::from_channel(endpoint.connect_lazy()))
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::time::Duration;

    use http::{HeaderMap, HeaderValue};

    use super::{LONGEST, timed};
    use crate::dispatch::grpc_timeout;

    const HOUR: Duration = Duration::from_secs(3600);

    /// What one unit of `unit` is.
    fn unit_of(unit: &str) -> Duration {
        match unit {
            "H" => HOUR,
            "M" => Duration::from_secs(60),
            "S" => Duration::from_secs(1),
            "m" => Duration::from_millis(1),
            "u" => Duration::from_micros(1),
            "n" => Duration::from_nanos(1),
            other => panic!("not a unit: {other:?}"),
        }
    }

    /// The named durations, then a pseudo-random sample spread over every magnitude a `Duration`
    /// holds, the same on every run.
    fn sweep() -> Vec<Duration> {
        let mut durations = vec![
            Duration::from_nanos(1),
            Duration::from_nanos(999),
            Duration::from_micros(1) + Duration::from_nanos(1),
            Duration::from_millis(1) - Duration::from_nanos(1),
            Duration::from_nanos(99_999_999),
            Duration::from_nanos(100_000_001),
            Duration::from_secs(1),
            Duration::from_secs(60),
            HOUR,
            HOUR * 24,
            HOUR * 24 * 100,
            LONGEST,
            LONGEST + Duration::from_nanos(1),
            LONGEST + HOUR,
            Duration::from_secs(u64::MAX / 1000),
            Duration::MAX,
        ];
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..2000 {
            let nanos = next() >> (next() % 64);
            durations.push(Duration::from_nanos(nanos.max(1)));
            let secs = next() >> (next() % 64);
            durations.push(Duration::new(secs, (next() % 1_000_000_000) as u32));
        }
        durations
    }

    /// What `outgoing` writes for `remaining`, decoded as the server reads it, or what went wrong.
    fn encode(remaining: Duration) -> Result<(String, Duration), String> {
        let request = catch_unwind(AssertUnwindSafe(|| timed(tonic::Request::new(()), remaining)))
            .map_err(|panic| format!("panicked: {:?}", panic.downcast_ref::<&str>().copied().or(panic.downcast_ref::<String>().map(String::as_str))))?;
        let value = request.metadata().get("grpc-timeout").ok_or("no `grpc-timeout` written")?.to_str().map_err(|error| error.to_string())?.to_owned();
        let (digits, unit) = value.split_at(value.len().saturating_sub(1));
        if digits.is_empty() || digits.len() > 8 || !digits.bytes().all(|byte| byte.is_ascii_digit()) || !"HMSmun".contains(unit) || unit.is_empty() {
            return Err(format!("{value:?} is off the grammar"));
        }
        let mut headers = HeaderMap::new();
        headers.insert("grpc-timeout", HeaderValue::from_str(&value).map_err(|error| error.to_string())?);
        match grpc_timeout(&headers) {
            Ok(Some(decoded)) => Ok((value, decoded)),
            other => Err(format!("{value:?} read back as {other:?}")),
        }
    }

    /// Every duration is written on the grammar, at most eight digits and one of `H M S m u n`,
    /// never as zero, and read back by the server no more than one unit away, or as the longest
    /// value the grammar holds when it is longer than that. A zero remainder is written as the
    /// shortest positive value.
    #[test]
    fn every_remaining_time_is_written_on_the_grammar_and_never_as_zero() {
        let mut failures = Vec::new();
        for remaining in sweep() {
            match encode(remaining) {
                Err(why) => failures.push(format!("{remaining:?}: {why}")),
                Ok((value, decoded)) => {
                    let unit = unit_of(&value[value.len() - 1..]);
                    let close = if remaining > LONGEST { decoded == LONGEST } else { remaining.abs_diff(decoded) < unit };
                    if decoded.is_zero() || !close {
                        failures.push(format!("{remaining:?}: written {value:?}, read back as {decoded:?}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{} of the sweep's durations failed, the first ten: {:#?}", failures.len(), &failures[..failures.len().min(10)]);
        assert_eq!(encode(Duration::ZERO).map(|(value, _)| value), Ok("1n".to_owned()), "a zero remainder");
    }
}
