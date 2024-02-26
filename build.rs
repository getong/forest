use proc_macro2::{Ident, Span};
use std::{env, error::Error, path::PathBuf};
use syn::{parse_quote, punctuated::Punctuated};

fn main() -> Result<(), Box<dyn Error>> {
    let mut impls = Vec::new();

    for num_tuples in 0..=12 {
        let (ty_params, var_names) = vars(num_tuples);

        // impl Vec2Tuple
        impls.push(parse_quote! {
            #[automatically_derived]
            impl<#(#ty_params: 'static),*> Vec2Tuple for (#(#ty_params,)*) {
                fn vec2tuple(mut vec: Vec<Box<dyn Any>>) -> Option<Self> {
                    vec.reverse();
                    #(let #var_names = vec.pop()?.downcast().ok()?;)*
                    match vec.is_empty() {
                        true => Some((#(*#var_names,)*)),
                        false => None,
                    }
                }
            }
        });

        let arity = ty_params.len();
        let ixs = 0..arity;

        // impl Dispatch
        impls.push(parse_quote! {
            #[automatically_derived]
            impl<F, #(#ty_params),*> Dispatch<#arity, (#(#ty_params,)*)> for F
            where
                F: fn_traits::FnOnce<(#(#ty_params,)*)>,
                #(#ty_params: for <'de> serde::Deserialize<'de> + 'static,)*
            {
                fn dispatch(
                    self,
                    wire_args: Option<RequestParameters>,
                    arg_names: [&str; #arity],
                ) -> Result<Self::Output, Error> {
                    let args = parse_args(
                        wire_args,
                        ParamStructure::Either,
                        &[
                            #(DynamicParamSpec::new::<#ty_params>(arg_names[#ixs]),)*
                        ],
                    )?;
                    match Vec2Tuple::vec2tuple(args) {
                        Some(args) => Ok(self.call_once(args)),
                        None => {
                            let msg = "mismatch in argument types or arity";
                            match cfg!(debug_assertions) {
                                true => panic!("{}", msg),
                                false => Err(Error::internal_error(msg, None)),
                            }
                        }
                    }
                }
            }
        });

        let ixs = 0..arity;

        // impl Describe
        impls.push(parse_quote! {
            #[automatically_derived]
            impl<'de, F, #(#ty_params),*> Describe<#arity, (#(#ty_params,)*)> for F
            where
                F: fn_traits::FnOnce<(#(#ty_params,)*)>,
                #(#ty_params: JsonSchema + Deserialize<'de>,)*
            {
                fn describe(
                    self,
                    arg_names: [&str; #arity],
                    gen: &mut SchemaGenerator,
                ) -> Result<Params, ParamListError> {
                    Params::new([
                        #(
                            ContentDescriptor {
                                name: arg_names[#ixs].into(),
                                schema: #ty_params::json_schema(gen),
                                required: !optional::<#ty_params>(),
                            }
                        ),*
                    ])
                }
            }
        });
    }

    let tuple_impls_rs = PathBuf::from(env::var("OUT_DIR")?).join("tuple_impls.rs");
    println!(
        "cargo:warning=tuple impls are at {}",
        tuple_impls_rs.display()
    );
    std::fs::write(
        tuple_impls_rs,
        prettyplease::unparse(&syn::File {
            shebang: None,
            attrs: Vec::new(),
            items: impls.into_iter().map(syn::Item::Impl).collect(),
        }),
    )?;

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
