use std::future::Future;

use schemars::{gen::SchemaGenerator, JsonSchema};
use serde::Deserialize;

use crate::jsonrpc_types::Error;
use crate::openrpc_types::{ContentDescriptor, Method, ParamStructure, Params};
use crate::util::Optional as _;

pub struct Signature {
    pub params: Params,
    pub calling_convention: ParamStructure,
    pub return_type: Option<ContentDescriptor>,
}

impl Signature {
    pub fn into_method(self, name: impl Into<String>) -> Method {
        let Self {
            params,
            calling_convention,
            return_type,
        } = self;
        Method {
            name: name.into(),
            params,
            param_structure: calling_convention,
            result: return_type,
        }
    }
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
