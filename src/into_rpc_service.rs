use std::future::Future;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    jsonrpc_types::{Error, RequestParameters},
    openrpc_types::ParamStructure,
    parser::{self, Parser},
    util::Optional as _,
};

/// `ARITY` must be a trait parameter rather than an associated constant because
/// the latter cannot be used in generic parameters.
///
/// `Args` must be a trait parameter so that fn arguments can be constrained in
/// the closure implementations.
///
/// # Panics
/// - Implementations may panic if [`parser::check_args`] fails, or [`Parser]'s
///   invariants are not upheld
pub trait IntoRpcService<const ARITY: usize, Args> {
    type RpcService: tower::Service<Option<RequestParameters>, Response = Value, Error = Error>;
    fn into_rpc_service(
        self,
        param_names: [&'static str; ARITY],
        calling_convention: ParamStructure,
    ) -> Self::RpcService;
}

fn serialize_response<T: Serialize>(it: T) -> Result<Value, Error> {
    serde_json::to_value(it).map_err(|e| {
        Error::internal_error(
            "couldn't serialize response object",
            json! {{
                "error": e.to_string(),
                "type": std::any::type_name::<T>()
            }},
        )
    })
}

include!(concat!(env!("OUT_DIR"), "/into_rpc_service.rs"));

#[cfg(test)]
mod tests {
    use crate::util::{call, examples, from_value};

    use super::*;

    #[test]
    fn simple_service() {
        let mut svc = examples::len.into_rpc_service(["string", "method"], ParamStructure::Either);

        // no args
        call(&mut svc, None).unwrap_err();
        call(&mut svc, from_value!([])).unwrap_err();
        call(&mut svc, from_value!({})).unwrap_err();

        // bad params
        call(&mut svc, from_value!([1])).unwrap_err();
        call(&mut svc, from_value!({"string": 1})).unwrap_err();

        call(&mut svc, from_value!(["𓀕", "bad"])).unwrap_err();
        call(&mut svc, from_value!({"string": "𓀕", "method": "bad"})).unwrap_err();

        // unexpected params
        call(&mut svc, from_value!(["𓀕", "bytes", "surpise"])).unwrap_err();
        call(
            &mut svc,
            from_value!({"string": "𓀕", "method": "bytes", "surprise": true}),
        )
        .unwrap_err();

        // required arg only
        assert_eq!(json!(4), call(&mut svc, from_value!(["𓀕"])).unwrap());
        assert_eq!(
            json!(4),
            call(&mut svc, from_value!({"string": "𓀕"})).unwrap()
        );

        // positional with optional arg
        assert_eq!(
            json!(4),
            call(&mut svc, from_value!(["𓀕", "bytes"])).unwrap()
        );
        assert_eq!(
            json!(1),
            call(&mut svc, from_value!(["𓀕", "chars"])).unwrap()
        );

        // named with optional arg
        assert_eq!(
            json!(4),
            call(&mut svc, from_value!({"string": "𓀕", "method": "bytes"})).unwrap()
        );
        assert_eq!(
            json!(1),
            call(&mut svc, from_value!({"string": "𓀕", "method": "chars"})).unwrap()
        );
    }

    #[test]
    #[should_panic = "mandatory param `string` follows optional param `method` at index 0"]
    fn bad_service() {
        examples::bad_len.into_rpc_service(["method", "string"], ParamStructure::Either);
    }
}
