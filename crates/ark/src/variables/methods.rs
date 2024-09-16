//
// methods.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use std::collections::HashMap;
use std::str::FromStr;

use anyhow::anyhow;
use harp::environment::r_ns_env;
use harp::environment::BindingValue;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::r_null;
use harp::r_symbol;
use harp::RObject;
use libr::Rf_lang3;
use libr::SEXP;
use stdext::result::ResultOrLog;
use strum::IntoEnumIterator;
use strum_macros::Display;
use strum_macros::EnumIter;
use strum_macros::EnumString;
use strum_macros::IntoStaticStr;

use crate::modules::ARK_ENVS;

#[derive(Debug, PartialEq, EnumString, EnumIter, IntoStaticStr, Display, Eq, Hash, Clone)]
pub enum ArkVariablesGenerics {
    #[strum(serialize = "ark_variable_display_value")]
    VariableDisplayValue,

    #[strum(serialize = "ark_variable_display_type")]
    VariableDisplayType,

    #[strum(serialize = "ark_variable_has_children")]
    VariableHasChildren,

    #[strum(serialize = "ark_variable_kind")]
    VariableKind,
}

impl ArkVariablesGenerics {
    fn register_method_from_package(
        generic: Self,
        class: &str,
        package: &str,
    ) -> anyhow::Result<()> {
        let method = RObject::from(unsafe {
            Rf_lang3(
                r_symbol!(":::"),
                r_symbol!(package),
                r_symbol!(format!("{generic}.{class}")),
            )
        });
        Self::register_method(generic, class, method)?;
        Ok(())
    }

    fn register_method(generic: Self, class: &str, method: RObject) -> anyhow::Result<()> {
        let generic_name: &str = generic.into();
        RFunction::new("", "register_ark_method")
            .add(RObject::try_from(generic_name)?)
            .add(RObject::try_from(class)?)
            .add(method)
            .call_in(ARK_ENVS.positron_ns)?;
        Ok(())
    }

    // Checks if a symbol name is a method and returns it's class
    fn parse_method(name: &String) -> Option<(Self, String)> {
        for method in ArkVariablesGenerics::iter() {
            let method_str: &str = method.clone().into();
            if name.starts_with::<&str>(method_str) {
                return Some((
                    method,
                    name.trim_start_matches::<&str>(method_str)
                        .trim_start_matches('.')
                        .to_string(),
                ));
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
        if let Some((generic, class)) = ArkVariablesGenerics::parse_method(&name) {
            ArkVariablesGenerics::register_method_from_package(generic, class.as_str(), package)?;
        }
    }

    Ok(())
}

pub fn dispatch_variables_method<T>(generic: ArkVariablesGenerics, x: SEXP) -> anyhow::Result<T>
where
    T: TryFrom<RObject>,
    <T as TryFrom<RObject>>::Error: std::fmt::Debug,
{
    dispatch_variables_method_with_args(generic, x, HashMap::new())
}

pub fn dispatch_variables_method_with_args<T>(
    generic: ArkVariablesGenerics,
    x: SEXP,
    args: HashMap<String, RObject>,
) -> anyhow::Result<T>
where
    T: TryFrom<RObject>,
    <T as TryFrom<RObject>>::Error: std::fmt::Debug,
{
    let generic_name: &str = generic.into();
    let mut call = RFunction::new("", "call_ark_method");

    call.add(generic_name);
    call.add(x);

    for (name, value) in args.into_iter() {
        call.param(name.as_str(), value);
    }

    match call.call_in(ARK_ENVS.positron_ns)?.try_into() {
        Ok(value) => Ok(value),
        Err(err) => Err(anyhow!("Failed converting to type: {err:?}")),
    }
}

#[harp::register]
extern "C" fn ps_register_ark_method(
    generic: SEXP,
    class: SEXP,
    method: SEXP,
) -> anyhow::Result<SEXP> {
    let generic: String = RObject::from(generic).try_into()?;
    let class: String = RObject::from(class).try_into()?;

    ArkVariablesGenerics::register_method(
        ArkVariablesGenerics::from_str(generic.as_str())?,
        class.as_str(),
        RObject::from(method),
    )?;
    Ok(r_null())
}
