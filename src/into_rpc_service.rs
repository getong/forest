use std::future::Future;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    jsonrpc_types::{Error, RequestParameters},
    openrpc_types::ParamStructure,
    parser::{self, Parser},
    util::Optional as _,
};

trait IntoRpcService<const ARITY: usize, Args> {
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
