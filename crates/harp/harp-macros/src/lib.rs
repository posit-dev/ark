//
// lib.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use proc_macro::TokenStream;
use quote::format_ident;
use quote::quote;
use quote::ToTokens;
use syn::parse_macro_input;
use syn::ItemStruct;

extern crate proc_macro;

fn invalid_parameter(stream: impl ToTokens) -> ! {
    panic!(
        "Invalid parameter `{}`: registered routines can only accept SEXP parameters.",
        stream.to_token_stream()
    );
}

fn invalid_extern(stream: impl ToTokens) -> ! {
    panic!(
        "Invalid signature `{}`: registered routines must be 'extern \"C\"'.",
        stream.to_token_stream()
    );
}

// TODO: Can we move more of this to the `Vector` trait?

#[proc_macro_attribute]
pub fn vector(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // TODO: How do we parse an attribute?

    // Parse input as struct.
    let data = parse_macro_input!(item as ItemStruct);

    // Get the name of the struct.
    let ident = data.ident.clone();

    // Include a bunch of derives.
    let all = quote! {

        #[derive(Debug)]
        #data

        impl std::ops::Deref for #ident {
            type Target = libr::SEXP;

            fn deref(&self) -> &Self::Target {
                &self.object.sexp
            }
        }

        impl std::ops::DerefMut for #ident {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.object.sexp
            }
        }

        impl std::convert::TryFrom<libr::SEXP> for #ident {
            type Error = crate::error::Error;
            fn try_from(value: libr::SEXP) -> Result<Self, Self::Error> {
                super::try_r_vector_from_r_sexp(value)
            }
        }

        impl<T> std::cmp::PartialEq<T> for #ident
        where
            T: crate::traits::AsSlice<<Self as Vector>::CompareType>
        {
            fn eq(&self, other: &T) -> bool {
                let other: &[<Self as Vector>::CompareType] = other.as_slice();

                let lhs = self.iter();
                let rhs = other.iter();
                let zipped = std::iter::zip(lhs, rhs);
                for (lhs, rhs) in zipped {
                    match lhs {
                        Some(lhs) => {
                            if lhs != *rhs {
                                return false;
                            }
                        }
                        None => {
                            return false;
                        }
                    }

                }

                true

            }
        }

    };

    all.into()
}

#[proc_macro_attribute]
pub fn register(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Get metadata about the function being registered.
    let mut function: syn::ItemFn = syn::parse(item).unwrap();

    // Make sure the function is 'extern "C"'.
    let abi = match function.sig.abi {
        Some(ref abi) => abi,
        None => invalid_extern(function.sig),
    };

    let name = match abi.name {
        Some(ref name) => name,
        None => invalid_extern(function.sig),
    };

    let name = name.to_token_stream().to_string();
    if name != "\"C\"" {
        invalid_extern(function.sig);
    }

    // Make sure that the function only accepts SEXPs.
    for input in function.sig.inputs.iter() {
        let pattern = match input {
            syn::FnArg::Typed(pattern) => pattern,
            syn::FnArg::Receiver(receiver) => invalid_parameter(receiver),
        };

        let stream = match *pattern.ty {
            syn::Type::Path(ref stream) => stream,
            _ => invalid_parameter(pattern),
        };

        let value = stream.into_token_stream().to_string();
        if value != "SEXP" {
            invalid_parameter(pattern);
        }
    }

    // Get the name from the attribute.
    let ident = function.sig.ident.clone();
    let nargs = function.sig.inputs.len() as i32;

    // Get the name (as a C string).
    let mut name = ident.to_string();
    name.push('\0');
    let name = syn::LitByteStr::new(name.as_bytes(), ident.span());

    // Give a name to the registration function.
    let register = format_ident!("_{}_call_method_def", ident);

    // Define a separate function that produces this for us.
    let registration = quote! {

        #[ctor::ctor]
        fn #register() {

            unsafe {
                harp::routines::add(libr::R_CallMethodDef {
                    name: (#name).as_ptr() as *const std::os::raw::c_char,
                    fun: Some(::std::mem::transmute(#ident as *const ())),
                    numArgs: #nargs
                });
            }

        }

    };

    // Wrap in `r_unwrap()` to convert Rust errors to R errors. To do this
    // we move the function block into an expanded expression and then
    // replace the block with this expression.
    let function_block = function.block;
    let output_type = function.sig.output.clone();

    let function_wrapper = quote! {
        // This must be a block so we can parse it back into `function.block`
        {

            // Convert Rust errors to R errors
            harp::exec::r_unwrap(|| #output_type {
                // Insulate from condition handlers and detect unexpected
                // errors/longjumps with a top-level context.
                //
                // TODO: This disables interrupts and `r_sandbox()` by
                // itself does not time out. We will want to do better.
                let result = harp::exec::r_sandbox(|| {
                    #function_block
                });

                result.unwrap_or_else(|err| {
                    panic!("Unexpected longjump while `.Call()`ing back into Ark: {err:?}");
                })
            })
        }
    };

    // Replace the original block with the expanded one
    function.block = syn::parse(function_wrapper.into()).unwrap();

    // Replace literal `Result<SEXP, _>` type by `SEXP` in the function
    // that we are actually registering. Type checking is performed by
    // `r_unwrap()` which takes the function body as input, ensuring that
    // it's a `Result<SEXP, _>` and guaranteeing that the expanded function
    // body does return a `SEXP` type.
    let sexp_type: syn::ReturnType = syn::parse(quote! { -> libr::SEXP }.into()).unwrap();
    function.sig.output = sexp_type;

    // Put everything together
    let all = quote! { #function #registration };
    all.into()
}
