use proc_macro2::{Ident, Span};
use std::{env, error::Error, ops::RangeInclusive, path::PathBuf};
use syn::{parse_quote, punctuated::Punctuated};

const NUM_TUPLES: RangeInclusive<usize> = 0..=12;

fn main() -> Result<(), Box<dyn Error>> {
    generate("signature.rs", || {
        NUM_TUPLES.map(|num_tuples| {
            let (ty_params, _) = vars(num_tuples);
            let arity = ty_params.len();
            let ixs = 0..arity;

            parse_quote! {
                #[automatically_derived]
                impl<'de, F, Fut, T, #(#ty_params,)*> GetReturningSignature<#arity, (#(#ty_params,)*)> for F
                where
                    F: FnOnce(#(#ty_params,)*) -> Fut,
                    Fut: Future<Output = Result<T, Error>>,
                    #(#ty_params: JsonSchema + Deserialize<'de>,)*
                    T: JsonSchema + Deserialize<'de>,
                {
                    fn get_returning_signature(
                        param_names: [&str; #arity],
                        return_name: &str,
                        calling_convention: ParamStructure,
                        gen: &mut SchemaGenerator,
                    ) -> Signature {
                        Signature {
                            params: Params::new([
                                #(content_descriptor::<#ty_params>(param_names[#ixs], gen),)*
                            ])
                            .unwrap(),
                            calling_convention,
                            return_type: Some(content_descriptor::<T>(return_name, gen)),
                        }
                    }
                }
            }
        })
    })?;

    generate("into_rpc_service.rs", || {
        NUM_TUPLES.map(|num_tuples| {
            let (ty_params, _) = vars(num_tuples);
            let arity = ty_params.len();

            parse_quote! {
                #[automatically_derived]
                impl<F, Fut, R, #(#ty_params,)*> IntoRpcService<#arity, (#(#ty_params,)*)> for F
                where
                    #(#ty_params: for <'de> Deserialize<'de> + Send,)*
                    F: Fn(#(#ty_params),*) -> Fut + Copy + Send, // TODO(aatifsyed): relax this bound
                    Fut: Future<Output = Result<R, Error>> + Send,
                    R: Serialize,
                    Self: 'static,
                {
                    type RpcService = tower::util::BoxCloneService<Option<RequestParameters>, Value, Error>;

                    fn into_rpc_service(
                        self,
                        param_names: [&'static str; #arity],
                        calling_convention: ParamStructure,
                    ) -> Self::RpcService {
                        parser::check_args(param_names, [#(#ty_params::optional(),)*]);
                        tower::util::BoxCloneService::new(tower::service_fn({
                            move |params: Option<RequestParameters>| async move {
                                #[allow(unused)]
                                let mut parser = Parser::new(params, &param_names, calling_convention)?;
                                self(#(parser.parse::<#ty_params>()?),*)
                                    .await
                                    .and_then(serialize_response)
                            }
                        }))
                    }
                }
            }
        })
    })?;

    Ok(())
}

/// ```ignore
/// (
///     [T0, T1, ...],
///     [t0, t1, ...]
/// )
/// ```
fn vars(num_tuples: usize) -> (Vec<syn::TypeParam>, Vec<syn::Ident>) {
    (
        (0..num_tuples)
            .map(|n| syn::TypeParam {
                attrs: Vec::new(),
                ident: Ident::new(&format!("T{}", n), Span::call_site()),
                colon_token: None,
                bounds: Punctuated::new(),
                eq_token: None,
                default: None,
            })
            .collect(),
        (0..num_tuples)
            .map(|n| Ident::new(&format!("t{}", n), Span::call_site()))
            .collect(),
    )
}

fn generate<I: IntoIterator<Item = syn::Item>>(
    key: &str,
    generator: impl FnOnce() -> I,
) -> Result<(), Box<dyn Error>> {
    let path = PathBuf::from(env::var("OUT_DIR")?).join(key);
    let ast = syn::File {
        shebang: None,
        attrs: vec![],
        items: generator().into_iter().collect(),
    };
    std::fs::write(path, prettyplease::unparse(&ast))?;
    Ok(())
}
