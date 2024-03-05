#[allow(unused)]
mod jsonrpc_types;
#[allow(unused)]
mod openrpc_types;

mod axum_like;
mod axum_like2;
mod axum_like3;
mod into_rpc_service;
mod parser;
mod signature;
mod towery;
mod util;

use into_rpc_service::IntoRpcService;
use openrpc_types::{ContentDescriptor, Method, ParamStructure};
use parser::Parser;
use schemars::{gen::SchemaGenerator, JsonSchema};
use signature::{GetReturningSignature, Signature};

use crate::util::Optional;

struct MyCtx<BS> {
    db: BS,
}

struct MyBlockstore {}

trait Blockstore {}
impl Blockstore for MyBlockstore {}
impl<T> Blockstore for &T where T: Blockstore {}

async fn concat<BS: Blockstore>(
    _ctx: &MyCtx<BS>,
    lhs: String,
    rhs: String,
) -> Result<String, jsonrpsee::types::ErrorObjectOwned> {
    Ok(lhs + &rhs)
}

struct WrappedModule<Ctx> {
    inner: jsonrpsee::server::RpcModule<Ctx>,
    schema_generator: SchemaGenerator,
    methods: Vec<Method>,
}

impl<BS> WrappedModule<MyCtx<BS>>
where
    BS: Blockstore + Send + Sync + 'static,
{
    fn serve0(&mut self) {
        let method_name = "concat";
        let calling_convention = ParamStructure::ByPosition;
        let param_names = ["lhs", "rhs"];
        let ret_name = "ret";
        type T0 = String;
        type T1 = String;
        type R = String;

        self.inner
            .register_async_method(method_name, move |params, ctx| async move {
                let params = params
                    .as_str()
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(|e| error2error(jsonrpc_types::Error::invalid_params(e, None)))?;
                let mut parser =
                    Parser::new(params, &param_names, calling_convention).map_err(error2error)?;
                concat(
                    &ctx,
                    parser.parse().map_err(error2error)?,
                    parser.parse().map_err(error2error)?,
                )
                .await
            })
            .unwrap();
        let method = Method {
            name: String::from(method_name),
            params: openrpc_types::Params::new([
                ContentDescriptor {
                    name: String::from(param_names[0]),
                    schema: T0::json_schema(&mut self.schema_generator),
                    required: T0::optional(),
                },
                ContentDescriptor {
                    name: String::from(param_names[1]),
                    schema: T1::json_schema(&mut self.schema_generator),
                    required: T1::optional(),
                },
            ])
            .unwrap(),
            param_structure: calling_convention,
            result: Some(ContentDescriptor {
                name: String::from(ret_name),
                schema: R::json_schema(&mut self.schema_generator),
                required: R::optional(),
            }),
        };
        self.methods.push(method);
    }
}

fn error2error(ours: jsonrpc_types::Error) -> jsonrpsee::types::ErrorObjectOwned {
    let jsonrpc_types::Error {
        code,
        message,
        data,
    } = ours;
    jsonrpsee::types::ErrorObject::owned(code as i32, message, data)
}

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
