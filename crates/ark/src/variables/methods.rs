//
// methods.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use anyhow::anyhow;
use harp::call::RArgument;
use harp::call::RCall;
use harp::environment::r_ns_env;
use harp::environment::BindingValue;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::r_null;
use harp::r_symbol;
use harp::utils::r_is_object;
use harp::RObject;
use libr::SEXP;
use stdext::result::ResultOrLog;
use strum::IntoEnumIterator;
use strum_macros::Display;
use strum_macros::EnumIter;
use strum_macros::EnumString;
use strum_macros::IntoStaticStr;

use crate::modules::ARK_ENVS;

#[derive(Debug, PartialEq, EnumString, EnumIter, IntoStaticStr, Display, Eq, Hash, Clone)]
pub enum ArkGenerics {
    #[strum(serialize = "ark_variable_display_value")]
    VariableDisplayValue,

    #[strum(serialize = "ark_variable_display_type")]
    VariableDisplayType,

    #[strum(serialize = "ark_variable_has_children")]
    VariableHasChildren,

    #[strum(serialize = "ark_variable_kind")]
    VariableKind,
}

impl ArkGenerics {
    // Dispatches the method on `x`
    // Returns
    //   - `None` if no method was found,
    //   - `Err` if method was found and errored
    //   - `Err`if the method result could not be coerced to `T`
    //   - T, if method was found and was succesfully executed
    pub fn try_dispatch<T>(&self, x: SEXP, args: Vec<RArgument>) -> anyhow::Result<Option<T>>
    where
        // Making this a generic allows us to handle the conversion to the expected output
        // type within the dispatch, which is much more ergonomic.
        T: TryFrom<RObject>,
        <T as TryFrom<RObject>>::Error: std::fmt::Debug,
    {
        if !r_is_object(x) {
            return Ok(None);
        }

        let generic: &str = self.into();
        let mut call = RFunction::new("", "call_ark_method");

        call.add(generic);
        call.add(x);

        for RArgument { name, value } in args.into_iter() {
            call.param(name.as_str(), value);
        }

        let result = call.call_in(ARK_ENVS.positron_ns)?;

        // No method for that object
        if result.sexp == r_null() {
            return Ok(None);
        }

        // Convert the result to the expected return type
        match result.try_into() {
            Ok(value) => Ok(Some(value)),
            Err(err) => Err(anyhow!("Conversion failed: {err:?}")),
        }
    }

    pub fn register_method(&self, class: &str, method: RObject) -> anyhow::Result<()> {
        let generic_name: &str = self.into();
        RFunction::new("", ".ps.register_ark_method")
            .add(RObject::try_from(generic_name)?)
            .add(RObject::try_from(class)?)
            .add(method)
            .call_in(ARK_ENVS.positron_ns)?;
        Ok(())
    }

    pub fn register_method_from_package(&self, class: &str, package: &str) -> anyhow::Result<()> {
        let method = RCall::new(unsafe { r_symbol!(":::") })
            .add(RObject::from(package))
            .add(RObject::from(format!("{self}.{class}")))
            .build();
        self.register_method(class, method)?;
        Ok(())
    }

    // Checks if a symbol name is a method and returns it's class
    fn parse_method(name: &String) -> Option<(Self, String)> {
        for method in ArkGenerics::iter() {
            let method_str: &str = method.clone().into();
            if name.starts_with::<&str>(method_str) {
                if let Some((_, class)) = name.split_once(".") {
                    return Some((method, class.to_string()));
                }
            }
        }
        None
    }
}

pub fn populate_methods_from_loaded_namespaces() -> anyhow::Result<()> {
    let loaded = RFunction::new("base", "loadedNamespaces").call()?;
    let loaded: Vec<String> = loaded.try_into()?;

    for pkg in loaded.into_iter() {
        populate_variable_methods_table(pkg.as_str()).or_log_error("Failed populating methods");
    }

    Ok(())
}

pub fn populate_variable_methods_table(package: &str) -> anyhow::Result<()> {
    let ns = r_ns_env(package)?;
    let symbol_names = ns
        .iter()
        .filter_map(Result::ok)
        .filter(|b| match b.value {
            BindingValue::Standard { .. } => true,
            BindingValue::Promise { .. } => true,
            _ => false,
        })
        .map(|b| -> String { b.name.into() });

    for name in symbol_names {
        if let Some((generic, class)) = ArkGenerics::parse_method(&name) {
            generic.register_method_from_package(class.as_str(), package)?;
        }
    }

    Ok(())
}
