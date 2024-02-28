use proc_macro2::{Ident, Span};
use std::{env, error::Error, path::PathBuf};
use syn::{parse_quote, punctuated::Punctuated};

fn main() -> Result<(), Box<dyn Error>> {
    let impls_rs = PathBuf::from(env::var("OUT_DIR")?).join("impl_get_returning_signature.rs");
    let mut impls = vec![];

    for num_tuples in 0..=12 {
        let (ty_params, _) = vars(num_tuples);
        let arity = ty_params.len();
        let ixs = 0..arity;

        impls.push(parse_quote! {
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
        });
    }

    std::fs::write(
        impls_rs,
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
