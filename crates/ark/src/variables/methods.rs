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

static ARK_VARIABLES_METHODS: Lazy<RwLock<HashMap<String, HashMap<String, String>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

fn register_variables_method(method: String, pkg: String, class: String) {
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

    let methods = vec![
        "ark_variable_display_value",
        "ark_variable_display_type",
        "ark_variable_has_children",
        "ark_variable_kind",
    ];

    for nm in symbol_names {
        for method in methods.clone() {
            if nm.starts_with(method) {
                register_variables_method(
                    String::from(method),
                    pkg.clone(),
                    // 1.. is used to remove the `.` that follows the method name
                    nm.trim_start_matches(format!("{method}.").as_str())
                        .to_string(),
                );
                break;
            }
        }
    }

    Ok(())
}

pub fn dispatch_variables_method<T>(method: String, x: SEXP) -> Option<T>
where
    T: TryFrom<RObject>,
{
    dispatch_variables_method_with_args(method, x, HashMap::new())
}

pub fn dispatch_variables_method_with_args<T>(
    method: String,
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
    method: String,
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
