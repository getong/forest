use std::{fmt::Display, future::Future};

use schemars::{gen::SchemaGenerator, JsonSchema};
use serde::{de::Visitor, forward_to_deserialize_any, Deserialize, Deserializer};

use crate::{
    jsonrpc_types::Error,
    openrpc_types::{ContentDescriptor, ParamStructure, Params},
};

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

include!(concat!(env!("OUT_DIR"), "/tuple_impls2.rs"));

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
