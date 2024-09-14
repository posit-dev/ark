//
// methods.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use std::collections::HashMap;
use std::sync::RwLock;

use anyhow::anyhow;
use harp::environment::r_ns_env;
use harp::environment::BindingValue;
use harp::environment::R_ENVS;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::utils::r_classes;
use harp::RObject;
use libr::SEXP;
use once_cell::sync::Lazy;
use stdext::result::ResultOrLog;
use strum::IntoEnumIterator;
use strum_macros::Display;
use strum_macros::EnumIter;
use strum_macros::EnumString;
use strum_macros::IntoStaticStr;

#[derive(Debug, PartialEq, EnumString, EnumIter, IntoStaticStr, Display, Eq, Hash, Clone)]
pub enum ArkVariablesMethods {
    #[strum(serialize = "ark_variable_display_value")]
    VariableDisplayValue,

    #[strum(serialize = "ark_variable_display_type")]
    VariableDisplayType,

    #[strum(serialize = "ark_variable_has_children")]
    VariableHasChildren,

    #[strum(serialize = "ark_variable_kind")]
    VariableKind,
}

impl ArkVariablesMethods {
    // Checks if a symbol name is a method and returns it's class
    fn parse_method(name: &String) -> Option<(Self, String)> {
        for method in ArkVariablesMethods::iter() {
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

static ARK_VARIABLES_METHODS: Lazy<RwLock<HashMap<ArkVariablesMethods, HashMap<String, String>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

fn register_variables_method(method: ArkVariablesMethods, pkg: String, class: String) {
    let mut tables = ARK_VARIABLES_METHODS.write().unwrap();
    log::info!("Found method:{method} for class:{class} on pkg:{pkg}");
    tables
        .entry(method)
        .or_insert_with(HashMap::new)
        .insert(class, pkg);
}

pub fn populate_methods_from_loaded_namespaces() -> anyhow::Result<()> {
    let loaded = RFunction::new("base", "loadedNamespaces").call()?;
    let loaded: Vec<String> = loaded.try_into()?;

    for pkg in loaded.into_iter() {
        populate_variable_methods_table(pkg).or_log_error("Failed populating methods");
    }

    Ok(())
}

pub fn populate_variable_methods_table(pkg: String) -> anyhow::Result<()> {
    let ns = r_ns_env(&pkg)?;
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
        if let Some((method, class)) = ArkVariablesMethods::parse_method(&name) {
            register_variables_method(method, pkg.clone(), class);
        }
    }

    Ok(())
}

pub fn dispatch_variables_method<T>(method: ArkVariablesMethods, x: SEXP) -> Option<T>
where
    T: TryFrom<RObject>,
{
    dispatch_variables_method_with_args(method, x, HashMap::new())
}

pub fn dispatch_variables_method_with_args<T>(
    method: ArkVariablesMethods,
    x: SEXP,
    args: HashMap<String, RObject>,
) -> Option<T>
where
    T: TryFrom<RObject>,
{
    // If the object doesn't have classes, just return None
    let classes: harp::vector::CharacterVector = r_classes(x)?;

    // Get the method table, if there isn't one return an empty string
    let tables = ARK_VARIABLES_METHODS.read().unwrap();
    let method_table = tables.get(&method)?;

    for class in classes.iter().filter_map(|x| x) {
        if let Some(pkg) = method_table.get(&class) {
            return match call_method(method.clone(), pkg.clone(), class.clone(), x, args.clone()) {
                Err(err) => {
                    log::warn!("Failed dispatching `{pkg}::{method}.{class}`: {err}");
                    continue; // Try the method for the next class if there's any
                },
                Ok(value) => Some(value),
            };
        }
    }
    None
}

fn call_method<T>(
    method: ArkVariablesMethods,
    pkg: String,
    class: String,
    x: SEXP,
    args: HashMap<String, RObject>,
) -> anyhow::Result<T>
where
    T: TryFrom<RObject>,
{
    let mut call = RFunction::new_internal(pkg.as_str(), format!("{method}.{class}").as_str());
    call.add(x);

    for (name, value) in args.into_iter() {
        call.param(name.as_str(), value);
    }

    let result = call.call_in(R_ENVS.global)?;

    match result.try_into() {
        Err(_) => Err(anyhow!("Failed converting to method return type.")),
        Ok(value) => Ok(value),
    }
}
