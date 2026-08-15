//! Attribute expansion for Rust-shaped Fil-C bridge declarations.

#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{Expr, ItemForeignMod, Lit, Meta, Token, parse_macro_input};

/// Replaces an `unsafe extern "Fil-C"` declaration with its generated safe
/// process-backed client.
///
/// The same source file must be passed to `filc_build::Config` from
/// the consuming crate's build script so the helper and client share one
/// declaration.
#[proc_macro_attribute]
pub fn bridge(arguments: TokenStream, item: TokenStream) -> TokenStream {
    let foreign = parse_macro_input!(item as ItemForeignMod);
    let arguments =
        parse_macro_input!(arguments with Punctuated::<Meta, Token![,]>::parse_terminated);

    match expand_bridge(&arguments, &foreign) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

fn expand_bridge(
    arguments: &Punctuated<Meta, Token![,]>,
    foreign: &ItemForeignMod,
) -> syn::Result<proc_macro2::TokenStream> {
    let abi = foreign
        .abi
        .name
        .as_ref()
        .map(syn::LitStr::value)
        .unwrap_or_default();
    if !matches!(abi.as_str(), "Fil-C" | "fil-c") {
        return Err(syn::Error::new_spanned(
            &foreign.abi,
            "a filc bridge must use the `Fil-C` ABI (lowercase `fil-c` is also accepted)",
        ));
    }
    if foreign.unsafety.is_none() {
        return Err(syn::Error::new_spanned(
            &foreign.abi,
            "write `unsafe extern \"Fil-C\"`; the block author must verify its declarations",
        ));
    }

    let name = string_argument(arguments, "name")?.ok_or_else(|| {
        syn::Error::new_spanned(&foreign.abi, "a filc bridge requires `name = \"...\"`")
    })?;
    let filename = format!("{name}.rs");

    Ok(quote! {
        include!(concat!(env!("OUT_DIR"), "/", #filename));
    })
}

fn string_argument(
    arguments: &Punctuated<Meta, Token![,]>,
    wanted: &str,
) -> syn::Result<Option<String>> {
    let mut result = None;
    for argument in arguments {
        let Meta::NameValue(name_value) = argument else {
            continue;
        };
        if !name_value.path.is_ident(wanted) {
            continue;
        }
        let Expr::Lit(expression) = &name_value.value else {
            return Err(syn::Error::new_spanned(
                &name_value.value,
                format!("`{wanted}` must be a string literal"),
            ));
        };
        let Lit::Str(value) = &expression.lit else {
            return Err(syn::Error::new_spanned(
                &expression.lit,
                format!("`{wanted}` must be a string literal"),
            ));
        };
        if result.replace(value.value()).is_some() {
            return Err(syn::Error::new_spanned(
                argument,
                format!("duplicate `{wanted}` argument"),
            ));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::expand_bridge;
    use syn::parse_quote;

    #[test]
    fn expands_a_literal_fil_c_block_to_generated_bindings() {
        let arguments = parse_quote!(name = "demo");
        let foreign = parse_quote! {
            unsafe extern "Fil-C" {
                pub fn add(left: i32, right: i32) -> i32;
            }
        };
        let output = expand_bridge(&arguments, &foreign).unwrap().to_string();
        assert!(output.contains("demo.rs"));
        assert!(!output.contains("extern"));
    }

    #[test]
    fn accepts_domens_lowercase_spelling() {
        let arguments = parse_quote!(name = "demo");
        let foreign = parse_quote! {
            unsafe extern "fil-c" {
                pub fn add(left: i32, right: i32) -> i32;
            }
        };
        expand_bridge(&arguments, &foreign).unwrap();
    }
}
