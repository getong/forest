#[allow(unused)]
mod jsonrpc_types;
#[allow(unused)]
mod openrpc_types;

mod into_rpc_service;
mod parser;
mod signature;
mod util;

use into_rpc_service::IntoRpcService;
use openrpc_types::ParamStructure;
use schemars::gen::SchemaGenerator;
use signature::{GetReturningSignature, Signature};

fn signature_and_service<const ARITY: usize, Handler, Args>(
    handler: Handler,
    param_names: [&'static str; ARITY],
    return_name: &str,
    calling_convention: ParamStructure,
    schema_generator: &mut SchemaGenerator,
) -> (Signature, Handler::RpcService)
where
    Handler: IntoRpcService<ARITY, Args>,
    Handler: GetReturningSignature<ARITY, Args>,
{
    let signature = Handler::get_returning_signature(
        param_names,
        return_name,
        calling_convention,
        schema_generator,
    );
    let service = handler.into_rpc_service(param_names, calling_convention);
    (signature, service)
}

#[cfg(test)]
mod tests {
    use super::*;

    use pretty_assertions::assert_eq;
    use schemars::gen::SchemaSettings;
    use serde_json::json;
    use util::{call, examples, from_value};

    /// User presents a freestanding function, and this library produces:
    /// - a [tower::Service] for that function, including parameter validation
    /// - an OpenRPC method definition
    #[test]
    fn test() {
        let mut gen = SchemaGenerator::new(SchemaSettings::openapi3());
        let (sig, mut svc) = signature_and_service(
            examples::len,
            ["string", "method"],
            "len",
            ParamStructure::Either,
            &mut gen,
        );
        assert_eq!(call(&mut svc, from_value!(["hello"])), Ok(json!(5)));
        assert_eq!(
            call(
                &mut svc,
                from_value!({"string": "hello", "method": "bytes"})
            ),
            Ok(json!(5))
        );
        assert_eq!(
            serde_json::to_value(sig.into_method("len")).unwrap(),
            json!({
                "name": "len",
                "paramStructure": "either",
                "params": [
                    {
                        "name": "string",
                        "required": true,
                        "schema": {
                            "type": "string"
                        }
                    },
                    {
                        "name": "method",
                        "required": false,
                        "schema": {
                            "$ref": "#/components/schemas/LenMethod",
                            "nullable": true
                        }
                    }
                ],
                "result": {
                    "name": "len",
                    "required": true,
                    "schema": {
                        "format": "uint",
                        "minimum": 0.0,
                        "type": "integer"
                    }
                }
            })
        );
        assert_eq!(
            serde_json::to_value(gen.definitions()).unwrap(),
            json!({
                "LenMethod": {
                    "enum": [
                        "bytes",
                        "chars"
                    ],
                    "type": "string"
                }
            })
        );
    }
}
