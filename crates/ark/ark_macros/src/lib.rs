//
// lib.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

//! Proc macros for the Ark kernel.
//!
//! ## `#[ark::register]`
//!
//! Registers a function as an R `.Call` entry point with automatic `Console`
//! access and panic safety. Composes with `#[harp::register]`.
//!
//! ```ignore
//! #[ark::register]
//! fn ps_my_function(console: &Console, x: SEXP) -> anyhow::Result<SEXP> {
//!     let dc = console.device_context();
//!     Ok(harp::r_null())
//! }
//! ```
//!
//! The macro transforms this into:
//!
//! ```ignore
//! #[harp::register]
//! unsafe extern "C-unwind" fn ps_my_function(x: SEXP) -> anyhow::Result<SEXP> {
//!     crate::console::Console::with(|console| {
//!         let dc = console.device_context();
//!         Ok(harp::r_null())
//!     })
//! }
//! ```
//!
//! `harp::register` then adds `r_unwrap()` (Rust error to R error),
//! `r_sandbox()` (catches R longjumps), and ctor-based routine registration.
//!
//! `Console::with()` catches Rust panics (e.g. from `RefCell` borrow
//! violations) and converts them to `anyhow::Error`, which `r_unwrap()`
//! surfaces as a clean R error instead of crashing the session.
//!
//! The first parameter may be `&Console` (any name, type is matched by
//! the last path segment). It is stripped from the generated C signature
//! and injected at runtime. All remaining parameters must be `SEXP`.
//!
//! The return type must be `anyhow::Result<SEXP>`.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;

extern crate proc_macro;

#[proc_macro_attribute]
pub fn register(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    match register_impl(function) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn register_impl(function: syn::ItemFn) -> syn::Result<proc_macro::TokenStream> {
    let span = function.sig.ident.span();

    // Partition parameters: optional leading `&Console` + remaining SEXP args.
    let mut console_ident: Option<syn::Ident> = None;
    let mut sexp_params: Vec<syn::FnArg> = Vec::new();

    for (i, param) in function.sig.inputs.iter().enumerate() {
        let typed = match param {
            syn::FnArg::Typed(t) => t,
            syn::FnArg::Receiver(r) => {
                return Err(syn::Error::new_spanned(
                    r,
                    "ark::register functions cannot have a `self` parameter",
                ));
            },
        };

        if i == 0 && is_ref_console(&typed.ty) {
            if let syn::Pat::Ident(pat) = &*typed.pat {
                console_ident = Some(pat.ident.clone());
            } else {
                console_ident = Some(syn::Ident::new("console", span));
            }
            continue;
        }

        if !is_sexp_type(&typed.ty) {
            return Err(syn::Error::new_spanned(
                &typed.ty,
                "ark::register parameters (other than the leading `&Console`) must be `SEXP`",
            ));
        }

        sexp_params.push(param.clone());
    }

    let ident = &function.sig.ident;
    let vis = &function.vis;
    let attrs = &function.attrs;
    let function_block = &function.block;

    // Build the body: wrap in `Console::with()` if `&Console` was requested,
    // otherwise just invoke the block directly.
    let body = if let Some(console_name) = console_ident {
        quote! {
            crate::console::Console::with(|#console_name| #function_block)
        }
    } else {
        quote! {
            (|| #function_block)()
        }
    };

    Ok(quote! {
        #(#attrs)*
        #[harp::register]
        #vis unsafe extern "C-unwind" fn #ident(#(#sexp_params),*) -> anyhow::Result<libr::SEXP> {
            #body
        }
    }
    .into())
}

/// Check if a type is `&Console` (matches `&Console` or `&path::to::Console`).
fn is_ref_console(ty: &syn::Type) -> bool {
    let syn::Type::Reference(ref_ty) = ty else {
        return false;
    };
    if ref_ty.mutability.is_some() {
        return false;
    }
    match &*ref_ty.elem {
        syn::Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "Console"),
        _ => false,
    }
}

/// Check if a type is `SEXP`.
fn is_sexp_type(ty: &syn::Type) -> bool {
    let syn::Type::Path(path) = ty else {
        return false;
    };
    path.path
        .segments
        .last()
        .is_some_and(|seg| seg.ident == "SEXP")
}
