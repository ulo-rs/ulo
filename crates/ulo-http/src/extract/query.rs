use std::ops::Deref;

use serde::de::DeserializeOwned;
use ulo::AppHandle;
use ulo_transport::{ExtractError, FieldViolation, FromCall, Validate};

use crate::cx::HttpCx;
use crate::extract::de::{Source, from_pairs, parse_form};
use crate::transport::Http;

/// The query string as `T`, decoded as `application/x-www-form-urlencoded`. A missing query is an
/// empty one, so a `T` whose fields are all optional or defaulted always extracts.
///
/// A required field the query lacks is `ExtractError::Missing` naming it, so `Option<Query<T>>` is
/// `None` then; a value that does not parse is `Malformed` naming it. Both are 400: the query is
/// part of the request's target rather than its content, so the 422 a body that does not fit its
/// type answers does not apply.
///
/// A name written more than once fills a `Vec` field with every value, in the order written, and a
/// name written once fills it with one: `?tag=a&tag=b` reads as `tag: vec!["a", "b"]`. Into a field
/// that is not a sequence a repeated name is `Malformed` naming the field, never its first or last
/// value. `Query<Vec<(String, String)>>` takes every pair as written, repeats included.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Query<T>(pub T);

impl<T> Deref for Query<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: DeserializeOwned + Send + 'static> FromCall<Http> for Query<T> {
    async fn from_call(cx: &HttpCx) -> Result<Self, ExtractError> {
        decode(cx.uri().query(), cx.app()).map(Query)
    }
}

/// `query`, absent read as empty, as `T` by [`Query`]'s rules, a failure redacted through `app`.
#[allow(clippy::result_large_err, reason = "the error type is public, so its size is an API decision rather than this function's")]
pub(crate) fn decode<T: DeserializeOwned>(query: Option<&str>, app: &AppHandle) -> Result<T, ExtractError> {
    let decoded = parse_form(query.unwrap_or("").as_bytes());
    let pairs: Vec<(&str, &str)> = decoded.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
    from_pairs::<T>(&pairs, Source::Form).map_err(|error| error.into_extract_in("query", app))
}

impl<T: Validate> Validate for Query<T> {
    fn validate(&self) -> Result<(), Vec<FieldViolation>> {
        self.0.validate()
    }
}
