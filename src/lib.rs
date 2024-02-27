#[allow(unused)]
mod jsonrpc_types;
#[allow(unused)]
mod openrpc_types;

mod lib2;

use std::{any::Any, collections::BTreeMap, convert::Infallible};

use itertools::Itertools as _;
use jsonrpc_types::{Error, Request, RequestParameters, Response};
use openrpc_types::{ContentDescriptor, ParamListError, ParamStructure, Params};
use schemars::{gen::SchemaGenerator, JsonSchema};
use serde::Deserialize;
use serde_json::{json, Value};

pub struct SelfDescribingService<T> {
    service: T,
    params: Params,
    calling_convention: ParamStructure,
    return_type: Option<ContentDescriptor>,
}

include!(concat!(env!("OUT_DIR"), "/tuple_impls.rs"));

pub async fn notify() {}

pub async fn hello() -> &'static str {
    "hello"
}

pub async fn len(string: String) -> usize {
    string.len()
}

pub async fn check(string: String) -> Result<(), &'static str> {
    match string == "hello" {
        true => Ok(()),
        false => Err("check failed"),
    }
}

pub async fn concat(left: String, right: String) -> String {
    left + &right
}

pub async fn handle(request: Request) -> Option<Response> {
    let Request {
        jsonrpc,
        method,
        params,
        id,
    } = request;
    match id {
        Some(id) => Some(match &*method {
            "hello" => match hello.dispatch(params, []) {
                Ok(job) => {
                    let res = job.await;
                    Response {
                        jsonrpc,
                        result: Ok(Value::from(res)),
                        id,
                    }
                }
                Err(e) => Response {
                    jsonrpc,
                    result: Err(e),
                    id,
                },
            },
            "len" => match len.dispatch(params, ["string"]) {
                Ok(job) => {
                    let res = job.await;
                    Response {
                        jsonrpc,
                        result: Ok(Value::from(res)),
                        id,
                    }
                }
                Err(e) => Response {
                    jsonrpc,
                    result: Err(e),
                    id,
                },
            },
            "check" => todo!(),
            "concat" => match concat.dispatch(params, ["left", "right"]) {
                Ok(job) => {
                    let res = job.await;
                    Response {
                        jsonrpc,
                        result: Ok(Value::from(res)),
                        id,
                    }
                }
                Err(e) => Response {
                    jsonrpc,
                    result: Err(e),
                    id,
                },
            },
            other => Response {
                jsonrpc,
                result: Err(Error::method_not_found(
                    format!("no such method `{}`", other),
                    json! {{
                        "available methods": []
                    }},
                )),
                id,
            },
        }),
        None => match &*method {
            "notify" => {
                notify().await;
                None
            }
            // Notifications are not confirmable by definition, since they do not have a Response object to be returned.
            // As such, the Client would not be aware of any errors (like e.g. "Invalid params","Internal error").
            _ => None,
        },
    }
}

#[test]
fn test() {
    let mut gen = SchemaGenerator::default();
    let params = concat.describe(["left", "right"], &mut gen).unwrap();
    let actual = serde_json::to_value(params).unwrap();

    assert_eq!(
        json!([
            {
                "name": "left",
                "required": true,
                "schema": {
                  "type": "string"
                }
              },
              {
                "name": "right",
                "required": true,
                "schema": {
                  "type": "string"
                }
              }
        ]),
        actual
    );
}

trait Describe<const ARITY: usize, Args>: fn_traits::FnOnce<Args> {
    fn describe(
        self,
        arg_names: [&str; ARITY],
        gen: &mut SchemaGenerator,
    ) -> Result<Params, ParamListError>;
}

// we need arity rather than e.g `trait TupleLen { const LEN: usize; }` because
// we're not allowed to reference `TupleLen::LEN` in the args
trait Dispatch<const ARITY: usize, Args>: fn_traits::FnOnce<Args> {
    fn dispatch(
        self,
        wire_args: Option<RequestParameters>,
        arg_names: [&str; ARITY],
    ) -> Result<Self::Output, Error>;
}

trait Vec2Tuple {
    fn vec2tuple(vec: Vec<Box<dyn Any>>) -> Option<Self>
    where
        Self: Sized;
}

#[derive(Clone, Copy)]
struct DynamicParamSpec<'a> {
    name: &'a str,
    optional: bool,
    deserialize: fn(Value) -> Result<Box<dyn Any>, serde_json::Error>,
}

fn optional<'de, T: Deserialize<'de>>() -> bool {
    use serde::de::Visitor;
    #[derive(Default)]
    struct DummyDeserializer;

    #[derive(thiserror::Error, Debug)]
    #[error("")]
    struct DeserializeOptionWasCalled(bool);

    impl serde::de::Error for DeserializeOptionWasCalled {
        fn custom<T: std::fmt::Display>(_: T) -> Self {
            Self(false)
        }
    }

    impl<'de> serde::Deserializer<'de> for DummyDeserializer {
        type Error = DeserializeOptionWasCalled;

        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
            Err(DeserializeOptionWasCalled(false))
        }

        fn deserialize_option<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
            Err(DeserializeOptionWasCalled(true))
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf unit unit_struct newtype_struct seq tuple
            tuple_struct map struct enum identifier ignored_any
        }
    }

    let Err(DeserializeOptionWasCalled(optional)) = T::deserialize(DummyDeserializer) else {
        unreachable!("DummyDeserializer never returns Ok(..)")
    };
    optional
}

impl<'a> DynamicParamSpec<'a> {
    fn new<T: for<'de> Deserialize<'de> + 'static>(name: &'a str) -> Self {
        Self {
            name,
            optional: optional::<T>(),
            deserialize: |it| T::deserialize(it).map(|it| Box::new(it) as Box<dyn Any>),
        }
    }
}

// it's simpler this way
fn parse_args(
    wire_args: Option<RequestParameters>,
    calling_convention: ParamStructure,
    specs: &[DynamicParamSpec<'_>],
) -> Result<Vec<Box<dyn Any>>, Error> {
    // spec: unique names
    let dups = specs.iter().map(|it| it.name).duplicates().collect_vec();
    if !dups.is_empty() {
        let msg = format!(
            "error at codegen site: the following parameter names are duplicated: [{}]",
            dups.join(", ")
        );
        match cfg!(debug_assertions) {
            true => panic!("{}", msg),
            false => return Err(Error::internal_error(msg, None)),
        }
    }

    // spec: mandatory parameters first
    if let Some((left, right)) = specs
        .iter()
        .tuple_windows()
        .find(|(left, right)| left.optional && !right.optional)
    {
        let msg = format!(
            "error at codegen site: mandatory parameter `{}` follows optional parameter `{}`",
            right.name, left.name
        );
        match cfg!(debug_assertions) {
            true => panic!("{}", msg),
            false => return Err(Error::internal_error(msg, None)),
        }
    };

    // common representation
    let mut ir = match (calling_convention, wire_args) {
        (
            ParamStructure::ByPosition | ParamStructure::Either,
            Some(RequestParameters::ByPosition(it)),
        ) => positional2ir(it, specs)?,
        (ParamStructure::ByName | ParamStructure::Either, Some(RequestParameters::ByName(it))) => {
            it.into_iter().map(|(k, v)| (k, (None, v))).collect()
        }
        (ParamStructure::ByName, Some(RequestParameters::ByPosition(_))) => {
            return Err(Error::invalid_params(
                "this method only accepts parameters by-name",
                None,
            ))
        }
        (ParamStructure::ByPosition, Some(RequestParameters::ByName(_))) => {
            return Err(Error::invalid_params(
                "this method only accepts parameters by-position",
                None,
            ))
        }
        (_, None) => BTreeMap::new(),
    };

    // parse the args
    let mut args = Vec::new();
    for spec in specs {
        let arg = match ir.remove(spec.name) {
            Some((ix, it)) => (spec.deserialize)(it).map_err(|it| {
                let mut msg = format!("error parsing argument `{}`", spec.name);
                if let Some(ix) = ix {
                    msg.push_str(&format!("at index {}", ix))
                }
                Error::invalid_params(msg, Value::String(it.to_string()))
            }),
            None => match spec.optional {
                false => Err(Error::invalid_params(
                    format!("mandatory argument `{}` was not provided", spec.name),
                    None,
                )),
                true => (spec.deserialize)(Value::Null).map_err(|e| {
                    let msg = format!(
                        "optional parameter `{}` could not be serialized from null: {}",
                        spec.name, e
                    );
                    match cfg!(debug_assertions) {
                        true => panic!("{}", msg),
                        false => Error::internal_error(msg, None),
                    }
                }),
            },
        };
        args.push(arg?);
    }

    match ir.is_empty() {
        true => Ok(args),
        false => Err(Error::invalid_params(
            format!("unexpected arguments: [{}]", ir.keys().join(", ")),
            None,
        )),
    }
}

fn positional2ir(
    args: Vec<Value>,
    specs: &[DynamicParamSpec<'_>],
) -> Result<BTreeMap<String, (Option<usize>, Value)>, Error> {
    use itertools::EitherOrBoth;
    let mut map = BTreeMap::new();
    for (ix, it) in args.into_iter().zip_longest(specs).enumerate() {
        match it {
            EitherOrBoth::Both(arg, spec) => {
                map.insert(spec.name.to_owned(), (Some(ix), arg));
            }
            EitherOrBoth::Left(_) => {
                return Err(Error::invalid_params(
                    format!("unexpected argument at position {}", ix),
                    None,
                ));
            }
            EitherOrBoth::Right(spec) => match spec.optional {
                true => {
                    return Err(Error::invalid_params(
                        format!(
                            "missing required parameter `{}` at position {}",
                            spec.name, ix
                        ),
                        None,
                    ))
                }
                false => continue,
            },
        }
    }

    Ok(map)
}
