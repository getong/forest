use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    fmt::Display,
    future::{self, Future, Ready},
    pin::Pin,
};

use futures::future::Either;
use itertools::Itertools;
use pin_project_lite::pin_project;
use schemars::{gen::SchemaGenerator, JsonSchema};
use serde::{de::Visitor, forward_to_deserialize_any, Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use std::task::{Context, Poll};
use tower::Service;

use crate::{
    jsonrpc_types::{Error, RequestParameters},
    openrpc_types::{ContentDescriptor, ParamStructure, Params},
    optional,
};

// trait IntoService

// trait Handler<const ARITY: usize, ConstrainArgs>: Clone + Send + Sized + 'static {
//     type Future: Future<Output = Result<Value, Error>> + Send + 'static;
//     const PARAM_NAMES: &'static [&'static str; ARITY];
//     const CALLING_CONVENTION: ParamStructure;
//     fn call(self, params: Option<RequestParameters>) -> Self::Future;
// }

// impl<F, Fut, R> Handler<0, ()> for F
// where
//     F: Clone + Send + Sized + 'static,
//     F: FnOnce() -> Fut + Clone,
//     Fut: Future<Output = Result<R, Error>> + Send + 'static,
//     R: Serialize,
// {
//     type Future = Fut;

//     const PARAM_NAMES: &'static [&'static str; 0];

//     const CALLING_CONVENTION: ParamStructure;

//     fn call(self, params: Option<RequestParameters>) -> Self::Future {
//         todo!()
//     }
// }

trait IntoRpcService<const ARITY: usize, Args> {
    type RpcService: tower::Service<Option<RequestParameters>, Response = Value, Error = Error>;
    fn into_rpc_service(
        self,
        param_names: [&'static str; ARITY],
        calling_convention: ParamStructure,
    ) -> Self::RpcService;
}

fn serialize_response(it: impl Serialize) -> Result<Value, Error> {
    serde_json::to_value(it).map_err(|e| {
        Error::internal_error(
            "couldn't serialize response object",
            json! {{
                "error": e.to_string()
            }},
        )
    })
}

impl<F, Fut, R> IntoRpcService<0, ()> for F
where
    F: Fn() -> Fut + Copy + Send, // TODO(aatifsyed): relax these bounds
    Fut: Future<Output = Result<R, Error>> + Send,
    R: Serialize,
    Self: 'static,
{
    type RpcService = tower::util::BoxService<Option<RequestParameters>, Value, Error>;

    fn into_rpc_service(self, _: [&'static str; 0], _: ParamStructure) -> Self::RpcService {
        tower::util::BoxService::new(tower::service_fn({
            move |params: Option<RequestParameters>| async move {
                match params.as_ref().map(RequestParameters::len) {
                    // lenient
                    None | Some(0) => self().await.and_then(serialize_response),
                    Some(n) => Err(Error::invalid_params(
                        "this method does not accept parameters",
                        json! {{
                            "number_of_params": n
                        }},
                    )),
                }
            }
        }))
    }
}

fn check_args<const N: usize>(names: [&str; N], optional: [bool; N]) {
    let duplicates = names.into_iter().duplicates().collect::<Vec<_>>();
    if !duplicates.is_empty() {
        panic!("duplicate param names: [{}]", duplicates.join(", "))
    }
    for (ix, (left, right)) in optional.into_iter().tuple_windows().enumerate() {
        if left && !right {
            panic!(
                "mandatory param {} follows optional param {} at index {}",
                names[ix + 1],
                names[ix],
                ix
            )
        }
    }
}

#[derive(Debug)]
struct Parser<'a> {
    params: Option<ParserInner>,
    /// What arguments do we expect to parse?
    argument_names: &'a [&'a str],
    /// How many times has the user called us so far?
    call_count: usize,
    has_errored: bool,
}

#[derive(Debug)]
enum ParserInner {
    ByPosition(VecDeque<Value>), // for O(1) pop_front
    ByName(serde_json::Map<String, Value>),
}

impl Drop for Parser<'_> {
    fn drop(&mut self) {
        if self.has_errored {
            return;
        }
        if !std::thread::panicking() {
            assert!(
                self.call_count >= self.argument_names.len(),
                "`Parser` has unhandled parameters - did you forget to call `parse`?"
            );
        }
    }
}

#[derive(Debug)]
enum ParseError<'a> {
    Missing {
        index: usize,
        name: &'a str,
        ty: &'a str,
    },
    Deser {
        index: usize,
        name: &'a str,
        ty: &'a str,
        error: serde_json::Error,
    },
    UnexpectedPositional(usize),
    UnexpectedNamed(Vec<String>),
    MustBeNamed,
    MustBePositional,
}

impl<'a> From<ParseError<'a>> for Error {
    fn from(value: ParseError<'a>) -> Self {
        match value {
            ParseError::Missing { index, name, ty } => Error::invalid_params(
                "missing required parameter",
                json!({
                    "index": index,
                    "name": name,
                    "type": ty
                }),
            ),
            ParseError::Deser {
                index,
                name,
                ty,
                error,
            } => Error::invalid_params(
                "error deserializing parameter",
                json!({
                    "index": index,
                    "name": name,
                    "type": ty,
                    "error": error.to_string()
                }),
            ),
            ParseError::UnexpectedPositional(n) => {
                Error::invalid_params("unexpected trailing arguments", json!({"count": n}))
            }
            ParseError::UnexpectedNamed(names) => {
                Error::invalid_params("unexpected named arguments", json!(names))
            }
            ParseError::MustBeNamed => {
                Error::invalid_params("this method only accepts arguments by-name", None)
            }
            ParseError::MustBePositional => {
                Error::invalid_params("this method only accepts arguments by-position", None)
            }
        }
    }
}

impl<'a> Parser<'a> {
    fn new(
        params: Option<RequestParameters>,
        names: &'a [&'a str],
        calling_convention: ParamStructure,
    ) -> Result<Self, ParseError> {
        let params = match (params, calling_convention) {
            // ignore the calling convention if there are no arguments to parse
            (None, _) => None,
            (Some(params), _) if names.is_empty() && params.is_empty() => None,
            // mutually exclusive
            (Some(RequestParameters::ByPosition(_)), ParamStructure::ByName) => {
                return Err(ParseError::MustBeNamed)
            }
            (Some(RequestParameters::ByName(_)), ParamStructure::ByPosition) => {
                return Err(ParseError::MustBePositional)
            }
            // `parse` won't be called, so do additional checks here
            (Some(RequestParameters::ByPosition(it)), _) if names.is_empty() && !it.is_empty() => {
                return Err(ParseError::UnexpectedPositional(it.len()))
            }
            (Some(RequestParameters::ByName(it)), _) if names.is_empty() && !it.is_empty() => {
                return Err(ParseError::UnexpectedNamed(
                    it.into_iter().map(|(it, _)| it).collect(),
                ))
            }
            (Some(RequestParameters::ByPosition(it)), _) => {
                Some(ParserInner::ByPosition(VecDeque::from(it)))
            }
            (Some(RequestParameters::ByName(it)), _) => Some(ParserInner::ByName(it)),
        };

        Ok(Self {
            params,
            argument_names: names,
            call_count: 0,
            has_errored: false,
        })
    }
    fn error<T>(&mut self, e: ParseError<'a>) -> Result<T, ParseError<'a>> {
        self.has_errored = true;
        Err(e)
    }
    fn parse<T>(&mut self) -> Result<T, ParseError<'a>>
    where
        T: for<'de> Deserialize<'de>,
    {
        let index = self.call_count;
        self.call_count += 1;
        let name = match self.argument_names.get(index) {
            Some(it) => *it,
            None => panic!(
                "`Parser` was initialized with {} arguments, but `parse` was called {} times",
                self.argument_names.len(),
                self.call_count
            ),
        };
        let ty = std::any::type_name::<T>();
        let missing_parameter = ParseError::Missing { index, name, ty };
        let deserialize_error = |error| ParseError::Deser {
            index,
            name,
            ty,
            error,
        };
        let t = match &mut self.params {
            None => match T::optional() {
                true => T::unwrap_none(),
                false => self.error(missing_parameter)?,
            },
            Some(ParserInner::ByName(it)) => match it.remove(name) {
                Some(it) => match serde_json::from_value::<T>(it) {
                    Ok(it) => it,
                    Err(e) => self.error(deserialize_error(e))?,
                },
                None => match T::optional() {
                    true => T::unwrap_none(),
                    false => self.error(missing_parameter)?,
                },
            },
            Some(ParserInner::ByPosition(it)) => match it.pop_front() {
                Some(it) => match serde_json::from_value::<T>(it) {
                    Ok(it) => it,
                    Err(e) => self.error(deserialize_error(e))?,
                },
                None => match T::optional() {
                    true => T::unwrap_none(),
                    false => self.error(missing_parameter)?,
                },
            },
        };
        let final_arg = self.call_count >= self.argument_names.len();
        if final_arg {
            match self.params.take() {
                Some(ParserInner::ByName(it)) => match it.is_empty() {
                    true => {}
                    false => self.error(ParseError::UnexpectedNamed(
                        it.into_iter().map(|(k, _)| k).collect(),
                    ))?,
                },
                Some(ParserInner::ByPosition(it)) => match it.len() {
                    0 => {}
                    n => self.error(ParseError::UnexpectedPositional(n))?,
                },
                None => {}
            };
        }
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! from_value {
        ($tt:tt) => {
            serde_json::from_value(serde_json::json!($tt)).unwrap()
        };
    }

    #[test]
    fn optional() {
        // no params where optional
        let mut parser = Parser::new(None, &["p0"], ParamStructure::Either).unwrap();
        assert_eq!(None::<i32>, parser.parse().unwrap());

        // positional optional
        let mut parser = Parser::new(from_value!([]), &["opt"], ParamStructure::Either).unwrap();
        assert_eq!(None::<i32>, parser.parse().unwrap());

        // named optional
        let mut parser = Parser::new(from_value!({}), &["opt"], ParamStructure::Either).unwrap();
        assert_eq!(None::<i32>, parser.parse().unwrap());

        // postional optional with mandatory
        let mut parser =
            Parser::new(from_value!([0]), &["p0", "opt"], ParamStructure::Either).unwrap();
        assert_eq!(Some(0), parser.parse().unwrap());
        assert_eq!(None::<i32>, parser.parse().unwrap());

        // named optional with mandatory
        let mut parser = Parser::new(
            from_value!({"p0": 0}),
            &["p0", "opt"],
            ParamStructure::Either,
        )
        .unwrap();
        assert_eq!(Some(0), parser.parse().unwrap());
        assert_eq!(None::<i32>, parser.parse().unwrap());
    }

    #[test]
    fn missing() {
        // missing only named
        let mut parser = Parser::new(from_value!({}), &["p0"], ParamStructure::Either).unwrap();
        assert!(matches!(
            parser.parse::<i32>().unwrap_err(),
            ParseError::Missing { name: "p0", .. },
        ));

        // missing only positional
        let mut parser = Parser::new(from_value!([]), &["p0"], ParamStructure::Either).unwrap();
        assert!(matches!(
            parser.parse::<i32>().unwrap_err(),
            ParseError::Missing { name: "p0", .. },
        ));

        // missing a named
        let mut parser = Parser::new(
            from_value!({"p0": 0}),
            &["p0", "p1"],
            ParamStructure::Either,
        )
        .unwrap();
        assert_eq!(0, parser.parse::<i32>().unwrap());
        assert!(matches!(
            parser.parse::<i32>().unwrap_err(),
            ParseError::Missing { name: "p1", .. },
        ));

        // missing a positional
        let mut parser =
            Parser::new(from_value!([0]), &["p0", "p1"], ParamStructure::Either).unwrap();
        assert_eq!(0, parser.parse::<i32>().unwrap());
        assert!(matches!(
            parser.parse::<i32>().unwrap_err(),
            ParseError::Missing { name: "p1", .. },
        ));
    }

    #[test]
    fn unexpected() {
        // named but expected none
        assert!(matches!(
            Parser::new(from_value!({ "surprise": () }), &[], ParamStructure::Either).unwrap_err(),
            ParseError::UnexpectedNamed(it) if it == ["surprise"],
        ));

        // positional but expected none
        assert!(matches!(
            Parser::new(from_value!(["surprise"]), &[], ParamStructure::Either).unwrap_err(),
            ParseError::UnexpectedPositional(1),
        ));

        // named after one
        let mut parser = Parser::new(
            from_value!({ "p0": 0, "surprise": () }),
            &["p0"],
            ParamStructure::Either,
        )
        .unwrap();
        assert!(matches!(
            parser.parse::<i32>().unwrap_err(),
            ParseError::UnexpectedNamed(it) if it == ["surprise"]
        ));

        // positional after one
        let mut parser = Parser::new(
            from_value!([1, "surprise"]),
            &["p0"],
            ParamStructure::Either,
        )
        .unwrap();
        assert!(matches!(
            parser.parse::<i32>().unwrap_err(),
            ParseError::UnexpectedPositional(1),
        ));
    }

    #[test]
    #[should_panic = "`Parser` was initialized with 0 arguments, but `parse` was called 1 times"]
    fn called_too_much() {
        let mut parser = Parser::new(None, &[], ParamStructure::Either).unwrap();
        let _ = parser.parse::<()>();
        unreachable!()
    }

    #[test]
    #[should_panic = "`Parser` has unhandled parameters - did you forget to call `parse`?"]
    fn called_too_little() {
        Parser::new(None, &["p0"], ParamStructure::Either).unwrap();
    }
}

impl<F, Fut, R, T0> IntoRpcService<1, (T0,)> for F
where
    F: Fn(T0) -> Fut + Copy + Send + Sync,
    T0: for<'de> Deserialize<'de> + Send,
    Fut: Future<Output = Result<R, Error>> + Send,
    R: Serialize,
    Self: 'static,
{
    type RpcService = tower::util::BoxService<Option<RequestParameters>, Value, Error>;

    fn into_rpc_service(
        self,
        names: [&'static str; 1],
        calling_convention: ParamStructure,
    ) -> Self::RpcService {
        check_args(names, [T0::optional()]);
        tower::util::BoxService::new(tower::service_fn(
            move |params: Option<RequestParameters>| async move {
                let mut args = Parser::new(params, &names, calling_convention)?;
                self(args.parse()?).await.and_then(serialize_response)
            },
        ))
    }
}

struct JsonRpcService<'a, const ARITY: usize, T> {
    inner: T,
    calling_convention: ParamStructure,
    param_names: [&'a str; ARITY],
}

impl<'a, F, Fut, T> Service<Option<RequestParameters>> for JsonRpcService<'a, 0, F>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Error>>,
    T: Serialize,
{
    type Response = Value;

    type Error = Error;

    type Future = Either<AndThenSerialize<Fut>, Ready<Result<Value, Error>>>;

    /// Always returns ready, like [`tower::util::ServiceFn`]
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Option<RequestParameters>) -> Self::Future {
        match req.map(|it| it.len()) {
            None | Some(0) => Either::Left(AndThenSerialize::new((self.inner)())),
            Some(n) => Either::Right(future::ready(Err(Error::invalid_params(
                format!("This function takes 0 arguments, but {} were supplied", n),
                None,
            )))),
        }
    }
}

pin_project! {
    struct AndThenSerialize<F> {
        #[pin]
        inner: F,
    }
}

impl<F> AndThenSerialize<F> {
    fn new(inner: F) -> Self {
        Self { inner }
    }
}

impl<T, F> Future for AndThenSerialize<F>
where
    F: Future<Output = Result<T, Error>>,
    T: Serialize,
{
    type Output = Result<Value, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().inner.poll(cx).map(|res| {
            res.and_then(|ok| {
                serde_json::to_value(ok).map_err(|e| {
                    Error::internal_error(
                        "error deserializing response from RPC handler",
                        json! {{
                            "deserialize_error": e.to_string(),
                            "type": std::any::type_name::<T>()
                        }},
                    )
                })
            })
        })
    }
}

trait ExtractParams<const ARITY: usize, Args> {
    type Output;
    fn extract_params(
        self,
        args: Option<RequestParameters>,
        param_names: [&str; ARITY],
    ) -> Result<Self::Output, Error>;
}

impl<F, T> ExtractParams<0, ()> for F
where
    F: FnOnce() -> T,
{
    type Output = T;

    fn extract_params(
        self,
        args: Option<RequestParameters>,
        param_names: [&str; 0],
    ) -> Result<Self::Output, Error> {
        Ok(self())
    }
}

impl<F, T, P0> ExtractParams<1, (P0,)> for F
where
    F: FnOnce(P0) -> T,
{
    type Output = T;

    fn extract_params(
        self,
        args: Option<RequestParameters>,
        param_names: [&str; 1],
    ) -> Result<Self::Output, Error> {
        todo!()
    }
}

struct Signature {
    params: Params,
    calling_convention: ParamStructure,
    return_type: Option<ContentDescriptor>,
}

/// `ARITY` must be a trait parameter rather than an associated constant because
/// the latter cannot be used in generic parameters.
///
/// `Args` must be a trait parameter so that fn arguments can be constrained in
/// the closure implementations.
///
/// # Panics
/// - Implementations may panic if argument ordering is incorrect
trait GetReturningSignature<const ARITY: usize, Args> {
    fn get_returning_signature(
        param_names: [&str; ARITY],
        return_name: &str,
        calling_convention: ParamStructure,
        gen: &mut SchemaGenerator,
    ) -> Signature;
}

fn content_descriptor<'de, T: JsonSchema + Deserialize<'de>>(
    name: &str,
    gen: &mut SchemaGenerator,
) -> ContentDescriptor {
    ContentDescriptor {
        name: String::from(name),
        schema: gen.subschema_for::<T>(),
        required: !T::optional(),
    }
}

include!(concat!(env!("OUT_DIR"), "/impl_get_returning_signature.rs"));

/// "Introspection" by tracing a [`Deserialize`] to see if it's [`Option`]-like.
///
/// This allows us to smoothly operate between rust functions that take an optional
/// paramater, and [`crate::openrpc_types::ContentDescriptor::required`].
trait Optional<'de>: Deserialize<'de> {
    fn optional() -> bool {
        #[derive(Default)]
        struct DummyDeserializer;

        #[derive(thiserror::Error, Debug)]
        #[error("")]
        struct DeserializeOptionWasCalled(bool);

        impl serde::de::Error for DeserializeOptionWasCalled {
            fn custom<T: Display>(_: T) -> Self {
                Self(false)
            }
        }

        impl<'de> Deserializer<'de> for DummyDeserializer {
            type Error = DeserializeOptionWasCalled;

            fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
                Err(DeserializeOptionWasCalled(false))
            }

            fn deserialize_option<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
                Err(DeserializeOptionWasCalled(true))
            }

            forward_to_deserialize_any! {
                bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
                bytes byte_buf unit unit_struct newtype_struct seq tuple
                tuple_struct map struct enum identifier ignored_any
            }
        }

        let Err(DeserializeOptionWasCalled(optional)) = Self::deserialize(DummyDeserializer) else {
            unreachable!("DummyDeserializer never returns Ok(..)")
        };
        optional
    }
    /// # Panics
    /// - This is only safe to call if [`Optional::optional`] returns `true`.
    fn unwrap_none() -> Self {
        Self::deserialize(serde_json::Value::Null)
            .expect("`null` json values should deserialize to a `None` for option-like types")
    }
}

impl<'de, T> Optional<'de> for T where T: Deserialize<'de> {}
