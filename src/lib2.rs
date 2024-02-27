use std::{
    fmt::Display,
    future::{self, Future, Ready},
    pin::Pin,
};

use futures::future::Either;
use pin_project_lite::pin_project;
use schemars::{gen::SchemaGenerator, JsonSchema};
use serde::{de::Visitor, forward_to_deserialize_any, Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use std::task::{Context, Poll};
use tower::Service;

use crate::{
    jsonrpc_types::{Error, RequestParameters},
    openrpc_types::{ContentDescriptor, ParamStructure, Params},
};

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
    fn none() -> Self {
        Self::deserialize(serde_json::Value::Null)
            .expect("`null` json values should deserialize to a `None` for option-like types")
    }
}

impl<'de, T> Optional<'de> for T where T: Deserialize<'de> {}
