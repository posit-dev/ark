//
// methods.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use anyhow::anyhow;
use harp::call::RArgument;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::r_null;
use harp::utils::r_is_object;
use harp::RObject;
use libr::SEXP;
use strum_macros::Display;
use strum_macros::EnumIter;
use strum_macros::EnumString;
use strum_macros::IntoStaticStr;

use crate::modules::ARK_ENVS;

#[derive(Debug, PartialEq, EnumString, EnumIter, IntoStaticStr, Display, Eq, Hash, Clone)]
pub enum ArkGenerics {
    #[strum(serialize = "ark_positron_variable_display_value")]
    VariableDisplayValue,

    #[strum(serialize = "ark_positron_variable_display_type")]
    VariableDisplayType,

    #[strum(serialize = "ark_positron_variable_has_children")]
    VariableHasChildren,

    #[strum(serialize = "ark_positron_variable_kind")]
    VariableKind,

    #[strum(serialize = "ark_positron_variable_get_child_at")]
    VariableGetChildAt,

    #[strum(serialize = "ark_positron_variable_get_children")]
    VariableGetChildren,
}

impl ArkGenerics {
    // Dispatches the method on `x`
    // Returns
    //   - `None` if no method was found,
    //   - `Err` if method was found and errored
    //   - `Err` if the method result could not be coerced to `T`
    //   - T, if method was found and was successfully executed
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
        RFunction::new("", ".ark.register_method")
            .add(RObject::try_from(generic_name)?)
            .add(RObject::try_from(class)?)
            .add(method)
            .call_in(ARK_ENVS.positron_ns)?;
        Ok(())
    }
}
