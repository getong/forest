pub mod jsonrpc_types;
pub mod openrpc_types;

mod parser;
mod util;

use crate::{
    jsonrpc_types::{Error, RequestParameters},
    util::Optional as _,
};
use jsonrpsee::{MethodsError, RpcModule};
use openrpc_types::{ContentDescriptor, Method, ParamStructure, Params};
use parser::Parser;
use schemars::{
    gen::{SchemaGenerator, SchemaSettings},
    schema::Schema,
    JsonSchema,
};
use serde::Serialize;
use serde::{
    de::{DeserializeOwned, Error as _, Unexpected},
    Deserialize,
};
use std::{any::Any, future::Future, sync::Arc};

pub struct SelfDescribingModule<Ctx> {
    inner: jsonrpsee::server::RpcModule<Ctx>,
    schema_generator: SchemaGenerator,
    calling_convention: ParamStructure,
    methods: Vec<Method>,
}

impl<Ctx> SelfDescribingModule<Ctx> {
    pub fn new(ctx: Ctx, calling_convention: ParamStructure) -> Self {
        Self {
            inner: jsonrpsee::server::RpcModule::new(ctx),
            schema_generator: SchemaGenerator::new(SchemaSettings::openapi3()),
            calling_convention,
            methods: vec![],
        }
    }
    pub fn register<'de, const ARITY: usize, T: RpcEndpoint<ARITY, Arc<Ctx>>>(
        &mut self,
    ) -> &mut Self
    where
        Ctx: Send + Sync + 'static,
        T::Ok: Serialize + Clone + 'static + JsonSchema + Deserialize<'de>,
    {
        self.register_with_calling_convention::<ARITY, T>(self.calling_convention)
    }
    pub fn register_with_calling_convention<
        'de,
        const ARITY: usize,
        T: RpcEndpoint<ARITY, Arc<Ctx>>,
    >(
        &mut self,
        override_cc: ParamStructure,
    ) -> &mut Self
    where
        Ctx: Send + Sync + 'static,
        T::Ok: Serialize + Clone + 'static + JsonSchema + Deserialize<'de>,
    {
        self.inner
            .register_async_method(T::METHOD_NAME, move |params, ctx| async move {
                let raw = params
                    .as_str()
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(|e| error2error(Error::invalid_params(e, None)))?;
                let args = T::Args::parse(raw, T::ARG_NAMES, override_cc).map_err(error2error)?;
                let ok = T::handle(ctx, args).await.map_err(error2error)?;
                Result::<_, jsonrpsee::types::ErrorObjectOwned>::Ok(ok)
            })
            .unwrap();

        let method = Method {
            name: String::from(T::METHOD_NAME),
            params: Params::new(
                itertools::zip_eq(T::ARG_NAMES, T::Args::schemas(&mut self.schema_generator)).map(
                    |(name, (schema, optional))| ContentDescriptor {
                        name: String::from(name),
                        schema,
                        required: !optional,
                    },
                ),
            )
            .unwrap(),
            param_structure: override_cc,
            result: Some(ContentDescriptor {
                name: format!("{}::Result", T::METHOD_NAME),
                schema: T::Ok::json_schema(&mut self.schema_generator),
                required: !T::Ok::optional(),
            }),
        };
        self.methods.push(method);
        self
    }

    pub fn finish(self) -> (jsonrpsee::server::RpcModule<Ctx>, openrpc_types::OpenRPC) {
        let Self {
            inner,
            mut schema_generator,
            methods,
            calling_convention: _,
        } = self;
        (
            inner,
            openrpc_types::OpenRPC {
                methods: openrpc_types::Methods::new(methods).unwrap(),
                components: openrpc_types::Components {
                    schemas: schema_generator.take_definitions().into_iter().collect(),
                },
            },
        )
    }

    pub async fn call<const ARITY: usize, T: RpcEndpoint<ARITY, Ctx>>(
        &self,
        args: T::Args,
    ) -> Result<T::Ok, jsonrpsee::MethodsError>
    where
        T::Args: Serialize,
        T::Ok: Clone + DeserializeOwned,
        Ctx: 'static,
    {
        self.call_with_calling_convention::<ARITY, T>(
            args,
            match self.calling_convention {
                ParamStructure::ByName | ParamStructure::Either => {
                    ConcreteCallingConvention::ByName
                }
                ParamStructure::ByPosition => ConcreteCallingConvention::ByPosition,
            },
        )
        .await
    }

    pub async fn call_with_calling_convention<const ARITY: usize, T: RpcEndpoint<ARITY, Ctx>>(
        &self,
        args: T::Args,
        override_cc: ConcreteCallingConvention,
    ) -> Result<T::Ok, jsonrpsee::MethodsError>
    where
        T::Args: Serialize,
        T::Ok: Clone + DeserializeOwned,
        Ctx: 'static,
    {
        call::<ARITY, Ctx, T>(&self.inner, args, override_cc).await
    }
}

pub async fn call<const ARITY: usize, Ctx, T: RpcEndpoint<ARITY, Ctx>>(
    module: &RpcModule<impl Any>,
    args: T::Args,
    calling_convention: ConcreteCallingConvention,
) -> Result<T::Ok, MethodsError>
where
    T::Args: Serialize,
    T::Ok: DeserializeOwned + Clone,
{
    match params::<ARITY, Ctx, T>(args, calling_convention)? {
        RequestParameters::ByPosition(args) => {
            let mut builder = jsonrpsee::core::params::ArrayParams::new();
            for arg in args {
                builder.insert(arg)?
            }
            module.call(T::METHOD_NAME, builder).await
        }
        RequestParameters::ByName(args) => {
            let mut builder = jsonrpsee::core::params::ObjectParams::new();
            for (name, value) in args {
                builder.insert(&name, value)?;
            }
            module.call(T::METHOD_NAME, builder).await
        }
    }
}

fn params<const ARITY: usize, Ctx, T: RpcEndpoint<ARITY, Ctx>>(
    args: T::Args,
    calling_convention: ConcreteCallingConvention,
) -> Result<RequestParameters, serde_json::Error>
where
    T::Args: Serialize,
{
    let args = args.unparse()?;
    match calling_convention {
        ConcreteCallingConvention::ByPosition => Ok(RequestParameters::ByPosition(Vec::from(args))),
        ConcreteCallingConvention::ByName => Ok(RequestParameters::ByName(
            itertools::zip_eq(T::ARG_NAMES.into_iter().map(String::from), args).collect(),
        )),
    }
}

pub enum ConcreteCallingConvention {
    ByPosition,
    ByName,
}

pub trait Args<const ARITY: usize> {
    fn schemas(gen: &mut SchemaGenerator) -> [(Schema, bool); ARITY];
    fn parse(
        raw: Option<RequestParameters>,
        arg_names: [&str; ARITY],
        calling_convention: ParamStructure,
    ) -> Result<Self, Error>
    where
        Self: Sized;
    fn unparse(&self) -> Result<[serde_json::Value; ARITY], serde_json::Error>
    where
        Self: Serialize,
    {
        fn kind(v: &serde_json::Value) -> Unexpected<'_> {
            match v {
                serde_json::Value::Null => Unexpected::Unit,
                serde_json::Value::Bool(it) => Unexpected::Bool(*it),
                serde_json::Value::Number(it) => match (it.as_f64(), it.as_i64(), it.as_u64()) {
                    (None, None, None) => Unexpected::Other("Number"),
                    (Some(it), _, _) => Unexpected::Float(it),
                    (_, Some(it), _) => Unexpected::Signed(it),
                    (_, _, Some(it)) => Unexpected::Unsigned(it),
                },
                serde_json::Value::String(it) => Unexpected::Str(it),
                serde_json::Value::Array(_) => Unexpected::Seq,
                serde_json::Value::Object(_) => Unexpected::Map,
            }
        }
        match serde_json::to_value(self) {
            Ok(serde_json::Value::Array(args)) => match args.try_into() {
                Ok(it) => Ok(it),
                Err(_) => Err(serde_json::Error::custom("ARITY mismatch")),
            },
            Ok(it) => Err(serde_json::Error::invalid_type(
                kind(&it),
                &"a Vec with an item for each argument",
            )),
            Err(e) => Err(e),
        }
    }
}

macro_rules! do_impls {
    ($arity:literal $(, $arg:ident)* $(,)?) => {
        const _: () = {
            let _assert: [&str; $arity] = [$(stringify!($arg)),*];
        };

        impl<$($arg),*> Args<$arity> for ($($arg,)*)
        where
            $($arg: DeserializeOwned + Serialize + JsonSchema),*
        {
            fn parse(
                raw: Option<RequestParameters>,
                arg_names: [&str; $arity],
                calling_convention: ParamStructure,
            ) -> Result<Self, Error> {
                let mut _parser = Parser::new(raw, &arg_names, calling_convention)?;
                Ok(($(_parser.parse::<$arg>()?,)*))
            }
            fn schemas(_gen: &mut SchemaGenerator) -> [(Schema, bool); $arity] {
                [$(($arg::json_schema(_gen), $arg::optional())),*]
            }
        }
    };
}

do_impls!(0);
do_impls!(1, T0);
do_impls!(2, T0, T1);
do_impls!(3, T0, T1, T2);
do_impls!(4, T0, T1, T2, T3);
do_impls!(5, T0, T1, T2, T3, T4);
do_impls!(6, T0, T1, T2, T3, T4, T5);
do_impls!(7, T0, T1, T2, T3, T4, T5, T6);
do_impls!(8, T0, T1, T2, T3, T4, T5, T6, T7);
do_impls!(9, T0, T1, T2, T3, T4, T5, T6, T7, T8);
do_impls!(10, T0, T1, T2, T3, T4, T5, T6, T7, T8, T9);

pub trait RpcEndpoint<const ARITY: usize, Ctx> {
    const METHOD_NAME: &'static str;
    const ARG_NAMES: [&'static str; ARITY];
    type Args: Args<ARITY>;
    type Ok;
    fn handle(ctx: Ctx, args: Self::Args) -> impl Future<Output = Result<Self::Ok, Error>> + Send;
}

fn error2error(ours: jsonrpc_types::Error) -> jsonrpsee::types::ErrorObjectOwned {
    let jsonrpc_types::Error {
        code,
        message,
        data,
    } = ours;
    jsonrpsee::types::ErrorObject::owned(code as i32, message, data)
}
