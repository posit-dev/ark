//
// variable.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use amalthea::comm::variables_comm::ClipboardFormatFormat;
use amalthea::comm::variables_comm::Variable;
use amalthea::comm::variables_comm::VariableKind;
use anyhow::anyhow;
use harp::call::RArgument;
use harp::environment::Binding;
use harp::environment::BindingValue;
use harp::environment::Environment;
use harp::environment::EnvironmentFilter;
use harp::error::Error;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::r_length;
use harp::object::RObject;
use harp::r_null;
use harp::r_symbol;
use harp::symbol::RSymbol;
use harp::utils::pairlist_size;
use harp::utils::r_altrep_class;
use harp::utils::r_assert_type;
use harp::utils::r_classes;
use harp::utils::r_inherits;
use harp::utils::r_is_altrep;
use harp::utils::r_is_data_frame;
use harp::utils::r_is_matrix;
use harp::utils::r_is_null;
use harp::utils::r_is_s4;
use harp::utils::r_is_simple_vector;
use harp::utils::r_is_unbound;
use harp::utils::r_promise_force_with_rollback;
use harp::utils::r_typeof;
use harp::utils::r_vec_is_single_dimension_with_single_value;
use harp::utils::r_vec_shape;
use harp::utils::r_vec_type;
use harp::vector::formatted_vector::FormattedVector;
use harp::vector::names::Names;
use harp::vector::CharacterVector;
use harp::vector::IntegerVector;
use harp::vector::Vector;
use harp::List;
use harp::TableDim;
use itertools::Itertools;
use libr::*;
use stdext::local;
use stdext::unwrap;

use crate::methods::ArkGenerics;

// Constants.
const MAX_DISPLAY_VALUE_ENTRIES: usize = 1_000;
const MAX_DISPLAY_VALUE_LENGTH: usize = 100;

pub struct WorkspaceVariableDisplayValue {
    pub display_value: String,
    pub is_truncated: bool,
}

fn plural(text: &str, n: i32) -> String {
    if n == 1 {
        String::from(text)
    } else {
        format!("{}s", text)
    }
}

impl WorkspaceVariableDisplayValue {
    pub fn from(value: SEXP) -> Self {
        // Try to use the display method if there's one available\
        if let Some(display_value) = Self::try_from_method(value) {
            return display_value;
        }

        match r_typeof(value) {
            NILSXP => Self::new(String::from("NULL"), false),
            VECSXP if r_inherits(value, "data.frame") => Self::from_data_frame(value),
            VECSXP if !r_inherits(value, "POSIXlt") => Self::from_list(value),
            LISTSXP => Self::empty(),
            SYMSXP if value == unsafe { R_MissingArg } => {
                Self::new(String::from("<missing>"), false)
            },
            CLOSXP => Self::from_closure(value),
            ENVSXP => Self::from_env(value),
            _ if r_is_matrix(value) => Self::from_matrix(value),
            _ => Self::from_default(value),
        }
    }

    fn new(display_value: String, is_truncated: bool) -> Self {
        WorkspaceVariableDisplayValue {
            display_value,
            is_truncated,
        }
    }

    fn empty() -> Self {
        Self::new(String::from(""), false)
    }

    fn from_data_frame(value: SEXP) -> Self {
        let dim = match unsafe { harp::df_dim(value) } {
            Ok(dim) => dim,
            // FIXME: Needs more type safety
            Err(_) => TableDim {
                num_rows: -1,
                num_cols: -1,
            },
        };

        let class = match r_classes(value) {
            None => String::from(""),
            Some(classes) => match classes.get_unchecked(0) {
                Some(class) => format!(" <{}>", class),
                None => String::from(""),
            },
        };

        let value = format!(
            "[{} {} x {} {}]{}",
            dim.num_rows,
            plural("row", dim.num_rows),
            dim.num_cols,
            plural("column", dim.num_cols),
            class
        );
        Self::new(value, false)
    }

    fn from_list(value: SEXP) -> Self {
        let n = r_length(value);
        let mut display_value = String::from("[");
        let mut is_truncated = false;
        let names = Names::new(value, |_i| String::from(""));

        for i in 0..n {
            if i > 0 {
                display_value.push_str(", ");
            }
            let display_i = Self::from(harp::list_get(value, i));
            let name = names.get_unchecked(i);
            if !name.is_empty() {
                display_value.push_str(&name);
                display_value.push_str(" = ");
            }
            display_value.push_str(&display_i.display_value);

            if display_value.len() > MAX_DISPLAY_VALUE_LENGTH || display_i.is_truncated {
                is_truncated = true;
            }
        }

        display_value.push(']');
        Self::new(display_value, is_truncated)
    }

    fn from_closure(value: SEXP) -> Self {
        unsafe {
            let args = RFunction::from("args").add(value).call().unwrap();
            let formatted = RFunction::from("format").add(*args).call().unwrap();
            let formatted = CharacterVector::new_unchecked(formatted);
            let out = formatted
                .iter()
                .take(formatted.len() - 1)
                .map(|o| o.unwrap())
                .join("");
            Self::new(out, false)
        }
    }

    fn from_env(value: SEXP) -> Self {
        // Get the environment and its length (excluding hidden bindings)
        let environment =
            Environment::new_filtered(RObject::view(value), EnvironmentFilter::ExcludeHidden);
        let environment_length = environment.length();

        // If the environment is empty, return the empty display value. If the environment is
        // large, return the large display value (because it may be too expensive to sort the
        // bindings). Otherwise, return a detailed display value that shows some or all of the
        // bindings in the environment.
        if environment_length == 0 {
            return Self::new(String::from("Empty Environment [0 values]"), false);
        }

        if environment_length > MAX_DISPLAY_VALUE_ENTRIES {
            return Self::new(
                format!("Large Environment [{} values]", environment_length),
                true,
            );
        }

        // For environment we don't display values, only names. So we don't need to create a
        // Variable for each bindings as we used to, and which caused an infinite recursion since
        // environments may be self-referential (posit-dev/positron#1690).
        let names = environment.names();

        // Build the detailed display value
        let mut display_value = String::new();

        let env_name = environment.name();
        if let Some(env_name) = env_name {
            display_value.push_str(format!("{env_name}: ").as_str())
        }

        display_value.push('{');

        let mut is_truncated = false;
        for (i, name) in names
            .iter()
            .filter(|name| !name.starts_with("."))
            .sorted_by(|lhs, rhs| Ord::cmp(&lhs, &rhs))
            .enumerate()
        {
            // If this isn't the first entry, append a space separator.
            if i > 0 {
                display_value.push_str(", ");
            }

            // Append the variable display name.
            display_value.push_str(name);

            // When the display value becomes too long, mark it as truncated and stop
            // building it.
            if i == 10 || display_value.len() > MAX_DISPLAY_VALUE_LENGTH {
                // If there are remaining entries, set the is_truncated flag and append a
                // counter of how many more entries there are.
                let remaining_entries = environment_length - 1 - i;
                if remaining_entries > 0 {
                    is_truncated = true;
                    display_value.push_str(&format!(" [{} more]", remaining_entries));
                }

                // Stop building the display value.
                break;
            }
        }

        display_value.push('}');

        // Return the display value.
        Self::new(display_value, is_truncated)
    }

    // TODO: handle higher dimensional arrays, i.e. expand
    //       recursively from the higher dimension
    fn from_matrix(value: SEXP) -> Self {
        let formatted = unwrap!(FormattedVector::new(value), Err(err) => {
            return Self::from_error(err);
        });

        let mut first = true;
        let mut display_value = String::from("");
        let mut is_truncated = false;

        unsafe {
            let dim = IntegerVector::new_unchecked(Rf_getAttrib(value, R_DimSymbol));
            let n_col = dim.get_unchecked(1).unwrap() as isize;
            display_value.push('[');
            for i in 0..n_col {
                if first {
                    first = false;
                } else {
                    display_value.push_str(", ");
                }

                display_value.push('[');
                let display_column = formatted.column_iter(i).join(" ");
                if display_column.len() > MAX_DISPLAY_VALUE_LENGTH {
                    is_truncated = true;
                    // TODO: maybe this should only push_str() a slice
                    //       of the first n (MAX_WIDTH?) characters in that case ?
                }
                display_value.push_str(display_column.as_str());
                display_value.push(']');

                if display_value.len() > MAX_DISPLAY_VALUE_LENGTH {
                    is_truncated = true;
                }
                if is_truncated {
                    break;
                }
            }
            display_value.push(']');
        }
        Self::new(display_value, is_truncated)
    }

    fn from_default(value: SEXP) -> Self {
        let formatted = unwrap!(FormattedVector::new(value), Err(err) => {
            return Self::from_error(err);
        });

        let mut first = true;
        let mut display_value = String::from("");
        let mut is_truncated = false;

        for x in formatted.iter() {
            if first {
                first = false;
            } else {
                display_value.push(' ');
            }
            display_value.push_str(&x);
            if display_value.len() > MAX_DISPLAY_VALUE_LENGTH {
                is_truncated = true;
                break;
            }
        }

        Self::new(display_value, is_truncated)
    }

    fn from_error(err: Error) -> Self {
        log::trace!("Error while formatting variable: {err:?}");
        Self::new(String::from("??"), true)
    }

    fn from_untruncated_string(mut value: String) -> Self {
        let Some((index, _)) = value.char_indices().nth(MAX_DISPLAY_VALUE_LENGTH) else {
            return Self::new(value, false);
        };

        // If an index is found, truncate the string to that index
        value.truncate(index);
        Self::new(value, true)
    }

    fn try_from_method(value: SEXP) -> Option<Self> {
        let display_value =
            ArkGenerics::VariableDisplayValue.try_dispatch::<String>(value, vec![RArgument::new(
                "width",
                RObject::from(MAX_DISPLAY_VALUE_LENGTH as i32),
            )]);

        let display_value = unwrap!(display_value, Err(err) => {
            log::error!("Failed to apply '{}': {err:?}", ArkGenerics::VariableDisplayValue.to_string());
            return None;
        });

        match display_value {
            None => None,
            Some(value) => Some(Self::from_untruncated_string(value)),
        }
    }
}

pub struct WorkspaceVariableDisplayType {
    pub display_type: String,
    pub type_info: String,
}

impl WorkspaceVariableDisplayType {
    /// Create a new WorkspaceVariableDisplayType from an R object.
    ///
    /// Parameters:
    /// - value: The R object to create the display type and type info for.
    /// - include_length: Whether to include the length of the object in the
    ///   display type.
    pub fn from(value: SEXP, include_length: bool) -> Self {
        match Self::try_from_method(value, include_length) {
            Err(err) => log::error!(
                "Error from '{}' method: {err}",
                ArkGenerics::VariableDisplayType.to_string()
            ),
            Ok(None) => {},
            Ok(Some(display_type)) => return display_type,
        }

        if r_is_null(value) {
            return Self::simple(String::from("NULL"));
        }

        if r_is_s4(value) {
            return Self::from_class(value, String::from("S4"));
        }

        if r_is_simple_vector(value) {
            let display_type = match include_length {
                true => match r_vec_is_single_dimension_with_single_value(value) {
                    true => r_vec_type(value),
                    false => format!("{} [{}]", r_vec_type(value), r_vec_shape(value)),
                },
                false => r_vec_type(value),
            };

            let mut type_info = display_type.clone();
            if r_is_altrep(value) {
                type_info.push_str(r_altrep_class(value).as_str())
            }

            return Self::new(display_type, type_info);
        }

        let rtype = r_typeof(value);
        match rtype {
            EXPRSXP => {
                let default = match include_length {
                    true => format!("expression [{}]", unsafe { Rf_xlength(value) }),
                    false => String::from("expression"),
                };
                Self::from_class(value, default)
            },
            LANGSXP => Self::from_class(value, String::from("language")),
            CLOSXP => Self::from_class(value, String::from("function")),
            ENVSXP => Self::from_class(value, String::from("environment")),
            SYMSXP => {
                if r_is_null(value) {
                    Self::simple(String::from("missing"))
                } else {
                    Self::simple(String::from("symbol"))
                }
            },

            LISTSXP => match include_length {
                true => match pairlist_size(value) {
                    Ok(n) => Self::simple(format!("pairlist [{}]", n)),
                    Err(_) => Self::simple(String::from("pairlist [?]")),
                },
                false => Self::simple(String::from("pairlist")),
            },

            VECSXP => unsafe {
                if r_is_data_frame(value) {
                    let classes = r_classes(value).unwrap();
                    let dfclass = classes.get_unchecked(0).unwrap();
                    match include_length {
                        true => {
                            let dim = RFunction::new("base", "dim.data.frame")
                                .add(value)
                                .call()
                                .unwrap();
                            let shape = FormattedVector::new(*dim).unwrap().iter().join(", ");
                            let display_type = format!("{} [{}]", dfclass, shape);
                            Self::simple(display_type)
                        },
                        false => Self::simple(dfclass),
                    }
                } else {
                    let default = match include_length {
                        true => format!("list [{}]", Rf_xlength(value)),
                        false => String::from("list"),
                    };
                    Self::from_class(value, default)
                }
            },
            _ => Self::from_class(value, String::from("???")),
        }
    }

    fn simple(display_type: String) -> Self {
        Self {
            display_type,
            type_info: String::from(""),
        }
    }

    fn from_class(value: SEXP, default: String) -> Self {
        match r_classes(value) {
            None => Self::simple(default),
            Some(classes) => Self::new(
                classes.get_unchecked(0).unwrap(),
                classes.iter().map(|s| s.unwrap()).join("/"),
            ),
        }
    }

    fn try_from_method(value: SEXP, include_length: bool) -> anyhow::Result<Option<Self>> {
        let args = vec![RArgument::new(
            "include_length",
            RObject::try_from(include_length)?,
        )];
        let result: Option<String> = ArkGenerics::VariableDisplayType.try_dispatch(value, args)?;
        Ok(result.map(Self::simple))
    }

    fn new(display_type: String, type_info: String) -> Self {
        Self {
            display_type,
            type_info,
        }
    }
}

fn has_children(value: SEXP) -> bool {
    match ArkGenerics::VariableHasChildren.try_dispatch(value, vec![]) {
        Err(err) => log::error!(
            "Error from '{}' method: {err}",
            ArkGenerics::VariableHasChildren.to_string()
        ),
        Ok(None) => {},
        Ok(Some(answer)) => return answer,
    }

    if RObject::view(value).is_s4() {
        unsafe {
            let names = RFunction::new("methods", ".slotNames")
                .add(value)
                .call()
                .unwrap();
            let names = CharacterVector::new_unchecked(names);
            names.len() > 0
        }
    } else {
        match r_typeof(value) {
            VECSXP | EXPRSXP => unsafe { Rf_xlength(value) != 0 },
            LISTSXP => true,
            ENVSXP => {
                !Environment::new_filtered(RObject::view(value), EnvironmentFilter::ExcludeHidden)
                    .is_empty()
            },
            LGLSXP | RAWSXP | STRSXP | INTSXP | REALSXP | CPLXSXP => unsafe {
                Rf_xlength(value) > 1
            },
            _ => false,
        }
    }
}

enum EnvironmentVariableNode {
    Concrete { object: RObject },
    R6Node { object: RObject, name: String },
    Matrixcolumn { object: RObject, index: isize },
    AtomicVectorElement { object: RObject, index: isize },
}

pub struct PositronVariable {
    var: Variable,
}

impl PositronVariable {
    /**
     * Create a new Variable from a Binding
     */
    pub fn new(binding: &Binding) -> Self {
        let display_name = binding.name.to_string();

        match &binding.value {
            BindingValue::Active { .. } => Self::from_active_binding(display_name),
            BindingValue::Promise { promise } => Self::from_promise(display_name, promise.sexp),
            BindingValue::Altrep { object, .. } | BindingValue::Standard { object, .. } => {
                Self::from(display_name.clone(), display_name, object.sexp)
            },
        }
    }

    /**
     * Create a new Variable from an R object
     */
    fn from(access_key: String, display_name: String, x: SEXP) -> Self {
        let WorkspaceVariableDisplayValue {
            display_value,
            is_truncated,
        } = WorkspaceVariableDisplayValue::from(x);
        let WorkspaceVariableDisplayType {
            display_type,
            type_info,
        } = WorkspaceVariableDisplayType::from(x, true);

        let kind = Self::variable_kind(x);

        let size = match RObject::view(x).size() {
            Ok(size) => size as i64,
            Err(err) => {
                log::warn!("Can't compute size of object: {err}");
                0
            },
        };

        Self {
            var: Variable {
                access_key,
                display_name,
                display_value,
                display_type,
                type_info,
                kind,
                length: Self::variable_length(x) as i64,
                size,
                has_children: has_children(x),
                is_truncated,
                has_viewer: r_is_data_frame(x) || r_is_matrix(x),
                updated_time: Self::update_timestamp(),
            },
        }
    }

    pub fn var(&self) -> Variable {
        self.var.clone()
    }

    fn from_promise(display_name: String, promise: SEXP) -> Self {
        let display_value = local! {
            unsafe {
                let code = PRCODE(promise);
                match r_typeof(code) {
                    SYMSXP => {
                        Ok(RSymbol::new_unchecked(code).to_string())
                    },
                    LANGSXP => {
                        let fun = RSymbol::new(CAR(code))?;
                        if fun == "lazyLoadDBfetch" {
                            return Ok(String::from("(unevaluated)"))
                        }
                        harp::call::expr_deparse_collapse(code)
                    },
                    _ => Err(Error::UnexpectedType(r_typeof(code), vec!(SYMSXP, LANGSXP)))
                }
            }
        };

        let display_value = match display_value {
            Ok(x) => x,
            Err(err) => {
                log::error!("{err}");
                String::from("(unevaluated)")
            },
        };

        Self {
            var: Variable {
                access_key: display_name.clone(),
                display_name,
                display_value,
                display_type: String::from("promise"),
                type_info: String::from("promise"),
                kind: VariableKind::Lazy,
                length: 0,
                size: 0,
                has_children: false,
                is_truncated: false,
                has_viewer: false,
                updated_time: Self::update_timestamp(),
            },
        }
    }

    fn from_active_binding(display_name: String) -> Self {
        Self {
            var: Variable {
                access_key: display_name.clone(),
                display_name,
                display_value: String::from(""),
                display_type: String::from("active binding"),
                type_info: String::from("active binding"),
                kind: VariableKind::Other,
                length: 0,
                size: 0,
                has_children: false,
                is_truncated: false,
                has_viewer: false,
                updated_time: Self::update_timestamp(),
            },
        }
    }

    fn variable_length(x: SEXP) -> usize {
        // Check for tabular data
        if let Some(info) = harp::table_info(x) {
            return info.dims.num_cols as usize;
        }

        // Otherwise treat as vector
        let rtype = r_typeof(x);
        match rtype {
            LGLSXP | RAWSXP | INTSXP | REALSXP | CPLXSXP | STRSXP | LISTSXP => unsafe {
                Rf_xlength(x) as usize
            },
            VECSXP => unsafe {
                // TODO: Support vctrs types like record vectors
                if r_inherits(x, "POSIXlt") && r_typeof(x) == VECSXP && r_length(x) > 0 {
                    Rf_xlength(VECTOR_ELT(x, 0)) as usize
                } else {
                    Rf_xlength(x) as usize
                }
            },
            _ => 0,
        }
    }

    fn variable_kind(x: SEXP) -> VariableKind {
        if x == unsafe { R_NilValue } {
            return VariableKind::Empty;
        }

        match try_from_method_variable_kind(x) {
            Err(err) => log::error!(
                "Error from '{}' method: {err}",
                ArkGenerics::VariableKind.to_string()
            ),
            Ok(None) => {},
            Ok(Some(kind)) => return kind,
        }

        let obj = RObject::view(x);

        if obj.is_s4() {
            return VariableKind::Map;
        }

        if r_inherits(x, "factor") {
            return VariableKind::Other;
        }

        if r_is_data_frame(x) {
            return VariableKind::Table;
        }

        // TODO: generic S3 object, not sure what it should be

        match r_typeof(x) {
            CLOSXP => VariableKind::Function,

            ENVSXP => {
                // this includes R6 objects
                VariableKind::Map
            },

            VECSXP => unsafe {
                let dim = Rf_getAttrib(x, R_DimSymbol);
                if dim != R_NilValue && Rf_xlength(dim) == 2 {
                    VariableKind::Table
                } else {
                    VariableKind::Map
                }
            },

            LGLSXP => unsafe {
                let dim = Rf_getAttrib(x, R_DimSymbol);
                if dim != R_NilValue && Rf_xlength(dim) == 2 {
                    VariableKind::Table
                } else if Rf_xlength(x) == 1 {
                    if LOGICAL_ELT(x, 0) == R_NaInt {
                        VariableKind::Empty
                    } else {
                        VariableKind::Boolean
                    }
                } else {
                    VariableKind::Collection
                }
            },

            INTSXP => unsafe {
                let dim = Rf_getAttrib(x, R_DimSymbol);
                if dim != R_NilValue && Rf_xlength(dim) == 2 {
                    VariableKind::Table
                } else if Rf_xlength(x) == 1 {
                    if INTEGER_ELT(x, 0) == R_NaInt {
                        VariableKind::Empty
                    } else {
                        VariableKind::Number
                    }
                } else {
                    VariableKind::Collection
                }
            },

            REALSXP => unsafe {
                let dim = Rf_getAttrib(x, R_DimSymbol);
                if dim != R_NilValue && Rf_xlength(dim) == 2 {
                    VariableKind::Table
                } else if Rf_xlength(x) == 1 {
                    if R_IsNA(REAL_ELT(x, 0)) == 1 {
                        VariableKind::Empty
                    } else {
                        VariableKind::Number
                    }
                } else {
                    VariableKind::Collection
                }
            },

            CPLXSXP => unsafe {
                let dim = Rf_getAttrib(x, R_DimSymbol);
                if dim != R_NilValue && Rf_xlength(dim) == 2 {
                    VariableKind::Table
                } else if Rf_xlength(x) == 1 {
                    let value = COMPLEX_ELT(x, 0);
                    if R_IsNA(value.r) == 1 || R_IsNA(value.i) == 1 {
                        VariableKind::Empty
                    } else {
                        VariableKind::Number
                    }
                } else {
                    VariableKind::Collection
                }
            },

            STRSXP => unsafe {
                let dim = Rf_getAttrib(x, R_DimSymbol);
                if dim != R_NilValue && Rf_xlength(dim) == 2 {
                    VariableKind::Table
                } else if Rf_xlength(x) == 1 {
                    if STRING_ELT(x, 0) == R_NaString {
                        VariableKind::Empty
                    } else {
                        VariableKind::String
                    }
                } else {
                    VariableKind::Collection
                }
            },

            RAWSXP => VariableKind::Bytes,
            _ => VariableKind::Other,
        }
    }

    pub fn inspect(env: RObject, path: &Vec<String>) -> Result<Vec<Variable>, harp::error::Error> {
        let node = Self::resolve_object_from_path(env, &path)?;

        match node {
            EnvironmentVariableNode::R6Node { object, name } => match name.as_str() {
                "<private>" => {
                    let env = Environment::new(object);
                    let enclos = Environment::new(RObject::view(env.find(".__enclos_env__")?));
                    let private = RObject::view(enclos.find("private")?);

                    Self::inspect_environment(private)
                },

                "<methods>" => Self::inspect_r6_methods(object),

                _ => Err(harp::error::Error::InspectError { path: path.clone() }),
            },

            EnvironmentVariableNode::Concrete { object } => {
                // First try to dispatch GetChildren method and construct
                // variables from it.
                match Self::try_inspect_custom_method(object.sexp) {
                    Err(err) => log::error!(
                        "Failed to inspect with {}: {err}",
                        ArkGenerics::VariableGetChildren.to_string()
                    ),
                    Ok(None) => {},
                    Ok(Some(variables)) => return Ok(variables),
                }

                if object.is_s4() {
                    Self::inspect_s4(*object)
                } else {
                    match r_typeof(*object) {
                        VECSXP | EXPRSXP => Self::inspect_list(*object),
                        LISTSXP => Self::inspect_pairlist(*object),
                        ENVSXP => {
                            if r_inherits(*object, "R6") {
                                Self::inspect_r6(object)
                            } else {
                                Self::inspect_environment(object)
                            }
                        },
                        LGLSXP | RAWSXP | STRSXP | INTSXP | REALSXP | CPLXSXP => {
                            if r_is_matrix(*object) {
                                Self::inspect_matrix(*object)
                            } else {
                                Self::inspect_vector(*object)
                            }
                        },
                        _ => Ok(vec![]),
                    }
                }
            },

            EnvironmentVariableNode::Matrixcolumn { object, index } => {
                Self::inspect_matrix_column(*object, index)
            },
            EnvironmentVariableNode::AtomicVectorElement { .. } => Ok(vec![]),
        }
    }

    pub fn clip(
        env: RObject,
        path: &Vec<String>,
        _format: &ClipboardFormatFormat,
    ) -> Result<String, harp::error::Error> {
        let node = Self::resolve_object_from_path(env, &path)?;

        match node {
            EnvironmentVariableNode::Concrete { object } => {
                if r_is_data_frame(*object) {
                    let formatted = RFunction::from(".ps.environment.clipboardFormatDataFrame")
                        .add(object)
                        .call()?;

                    Ok(FormattedVector::new(*formatted)?.iter().join("\n"))
                } else if r_typeof(*object) == CLOSXP {
                    let deparsed: Vec<String> =
                        RFunction::from("deparse").add(*object).call()?.try_into()?;

                    Ok(deparsed.join("\n"))
                } else {
                    Ok(FormattedVector::new(*object)?.iter().join(" "))
                }
            },
            EnvironmentVariableNode::R6Node { .. } => Ok(String::from("")),
            EnvironmentVariableNode::AtomicVectorElement { object, index } => {
                let formatted = FormattedVector::new(*object)?;
                Ok(formatted.get_unchecked(index))
            },
            EnvironmentVariableNode::Matrixcolumn { object, index } => unsafe {
                let dim = IntegerVector::new(Rf_getAttrib(*object, R_DimSymbol))?;
                let n_row = dim.get_unchecked(0).unwrap() as usize;

                let clipped = FormattedVector::new(*object)?
                    .iter()
                    .skip(index as usize * n_row)
                    .take(n_row)
                    .join(" ");
                Ok(clipped)
            },
        }
    }

    pub fn resolve_data_object(
        env: RObject,
        path: &Vec<String>,
    ) -> Result<RObject, harp::error::Error> {
        let resolved = Self::resolve_object_from_path(env, path)?;

        match resolved {
            EnvironmentVariableNode::Concrete { object } => Ok(object),

            _ => Err(harp::error::Error::InspectError { path: path.clone() }),
        }
    }

    fn get_envsxp_child_node_at(
        object: RObject,
        access_key: &String,
    ) -> harp::Result<EnvironmentVariableNode> {
        let symbol = unsafe { r_symbol!(access_key) };
        let mut x = unsafe { Rf_findVarInFrame(*object, symbol) };

        if r_typeof(x) == PROMSXP {
            // if we are here, it means the promise is either evaluated
            // already, i.e. PRVALUE() is bound or it is a promise to
            // something that is not a call or a symbol because it would
            // have been handled in Binding::new()

            // Actual promises, i.e. unevaluated promises can't be
            // expanded in the variables pane so we would not get here.

            let value = unsafe { PRVALUE(x) };
            if r_is_unbound(value) {
                x = unsafe { PRCODE(x) };
            } else {
                x = value;
            }
        }

        Ok(EnvironmentVariableNode::Concrete {
            object: RObject::view(x),
        })
    }

    fn get_concrete_child_node(
        object: RObject,
        access_key: &String,
    ) -> harp::Result<EnvironmentVariableNode> {
        // Concrete nodes are objects that are treated as is. Accessing an element from them
        // might result in special node types.

        // First try to get child using a generic method
        // When building the children list of nodes that use a custom `get_children` method, the access_key is
        // formatted as "custom-{index}-{length(name)}-{name}". If the access_key has this format, we call the custom `get_child_at`,
        // method, if there's one available:
        let result = local!({
            let parsed_access_key: Vec<&str> = access_key.splitn(4, '-').collect();

            if parsed_access_key.len() != 4 {
                return Ok(None);
            };

            if parsed_access_key[0] != "custom" {
                return Ok(None);
            };

            let index = match parsed_access_key[1].parse::<i32>() {
                Err(_) => return Ok(None), // Not an access_key in the required format
                Ok(i) => i,
            };

            let name_len = match parsed_access_key[2].parse::<usize>() {
                Err(_) => return Ok(None), // Not an access_key in the required format
                Ok(name_len) => name_len,
            };

            let name = match parsed_access_key[3] {
                "" => RObject::from(r_null()), // Empty string, means a `NULL` name
                nm => {
                    if nm.len() == name_len {
                        RObject::from(nm)
                    } else {
                        // Name has been truncated, we pass it as `NULL`
                        RObject::from(r_null())
                    }
                },
            };

            ArkGenerics::VariableGetChildAt.try_dispatch::<RObject>(object.sexp, vec![
                RArgument::new("index", RObject::from(index + 1)), // Index is 0-based, so we convert to 1-based for R.
                RArgument::new("name", RObject::from(name)),
            ])
        });

        match result {
            Err(err) => {
                // It's not safe to apply default methods in this case, because we rely on custom
                // access keys, which could indicate the access index depending on the node implementation.
                // See for instance, how it's used to index lists and atomic vectors.
                return Err(harp::Error::Anyhow(err));
            },
            Ok(None) => {
                // The object doesn't have a custom get_child_at method. We apply
                // the default built-in methods.
            },
            Ok(Some(child)) => return Ok(EnvironmentVariableNode::Concrete { object: child }),
        };

        // For S4 objects, we acess child nodes using R_do_slot.
        if object.is_s4() {
            let name = unsafe { r_symbol!(access_key) };
            let child: RObject =
                harp::try_catch(|| unsafe { R_do_slot(object.sexp, name) }.into())?;
            return Ok(EnvironmentVariableNode::Concrete { object: child });
        }

        // R6 objects may be accessed with special elements called <methods> and <private>.
        // For them, we'll have to build the next node artifically.
        if r_inherits(object.sexp, "R6") && access_key.starts_with("<") {
            return Ok(EnvironmentVariableNode::R6Node {
                object,
                name: access_key.clone(),
            });
        }

        match r_typeof(*object) {
            ENVSXP => Self::get_envsxp_child_node_at(object, access_key),
            VECSXP | EXPRSXP => {
                let index = parse_index(access_key)?;
                Ok(EnvironmentVariableNode::Concrete {
                    object: RObject::view(harp::list_get(object.sexp, index)),
                })
            },
            LISTSXP => {
                let mut pairlist = *object;
                let index = parse_index(access_key)?;
                for _i in 0..index {
                    pairlist = unsafe { CDR(pairlist) };
                }
                Ok(EnvironmentVariableNode::Concrete {
                    object: RObject::view(unsafe { CAR(pairlist) }),
                })
            },
            LGLSXP | RAWSXP | STRSXP | INTSXP | REALSXP | CPLXSXP => {
                if r_is_matrix(object.sexp) {
                    Ok(EnvironmentVariableNode::Matrixcolumn {
                        object,
                        index: parse_index(access_key)?,
                    })
                } else {
                    Ok(EnvironmentVariableNode::AtomicVectorElement {
                        object,
                        index: parse_index(access_key)?,
                    })
                }
            },
            _ => Err(harp::Error::Anyhow(anyhow!(
                "Unexpected child at {access_key}"
            ))),
        }
    }

    fn get_child_node_at(
        node: EnvironmentVariableNode,
        path_elt: &String,
    ) -> harp::Result<EnvironmentVariableNode> {
        match node {
            EnvironmentVariableNode::Concrete { object } => {
                Self::get_concrete_child_node(object, path_elt)
            },

            EnvironmentVariableNode::R6Node { object, name } => {
                match name.as_str() {
                    "<private>" => {
                        let env = Environment::new(object);
                        let enclos = Environment::new(RObject::view(env.find(".__enclos_env__")?));
                        let private = Environment::new(RObject::view(enclos.find("private")?));

                        // TODO: it seems unlikely that private would host active bindings
                        //       so find() is fine, we can assume this is concrete
                        Ok(EnvironmentVariableNode::Concrete {
                            object: RObject::view(private.find(path_elt)?),
                        })
                    },

                    _ => {
                        // Technically we'd also implement this for `<methods>`, but because `methods`
                        // are all functions which always `have_children=false` we don't need to.
                        return Err(harp::Error::Anyhow(anyhow!(
                            "You can only get children from <private>, got {path_elt}"
                        )));
                    },
                }
            },

            EnvironmentVariableNode::AtomicVectorElement { .. } => {
                return Err(harp::Error::Anyhow(anyhow!(
                    "Can't subset an atomic vector even further, got {path_elt}"
                )));
            },

            EnvironmentVariableNode::Matrixcolumn { object, index } => unsafe {
                let dim = IntegerVector::new(Rf_getAttrib(*object, R_DimSymbol))?;
                let n_row = dim.get_unchecked(0).unwrap() as isize;

                // TODO: use ? here, but this does not return a crate::error::Error, so
                //       maybe use anyhow here instead ?
                let row_index = path_elt.parse::<isize>().unwrap();

                Ok(EnvironmentVariableNode::AtomicVectorElement {
                    object,
                    index: n_row * index + row_index,
                })
            },
        }
    }

    fn resolve_object_from_path(
        object: RObject,
        path: &Vec<String>,
    ) -> harp::Result<EnvironmentVariableNode> {
        let mut node = EnvironmentVariableNode::Concrete { object };

        for path_elt in path {
            node = Self::get_child_node_at(node, path_elt)?
        }

        Ok(node)
    }

    fn inspect_list(value: SEXP) -> Result<Vec<Variable>, harp::error::Error> {
        let mut out: Vec<Variable> = vec![];
        let n = unsafe { Rf_xlength(value) };

        let names = Names::new(value, |i| format!("[[{}]]", i + 1));

        for i in 0..n {
            let obj = unsafe { VECTOR_ELT(value, i) };
            out.push(Self::from(i.to_string(), names.get_unchecked(i), obj).var());
        }

        Ok(out)
    }

    fn inspect_matrix(matrix: SEXP) -> harp::error::Result<Vec<Variable>> {
        unsafe {
            let matrix = RObject::new(matrix);
            let dim = IntegerVector::new(Rf_getAttrib(*matrix, R_DimSymbol))?;

            let n_col = dim.get_unchecked(1).unwrap();

            let mut out: Vec<Variable> = vec![];
            let formatted = FormattedVector::new(*matrix)?;

            for i in 0..n_col {
                let display_value = format!("[{}]", formatted.column_iter(i as isize).join(", "));
                out.push(Variable {
                    access_key: format!("{}", i),
                    display_name: format!("[, {}]", i + 1),
                    display_value,
                    display_type: String::from(""),
                    type_info: String::from(""),
                    kind: VariableKind::Collection,
                    length: 1,
                    size: 0,
                    has_children: true,
                    is_truncated: false,
                    has_viewer: false,
                    updated_time: Self::update_timestamp(),
                });
            }

            Ok(out)
        }
    }

    fn inspect_matrix_column(matrix: SEXP, index: isize) -> harp::error::Result<Vec<Variable>> {
        unsafe {
            let matrix = RObject::new(matrix);
            let dim = IntegerVector::new(Rf_getAttrib(*matrix, R_DimSymbol))?;

            let n_row = dim.get_unchecked(0).unwrap();

            let mut out: Vec<Variable> = vec![];
            let formatted = FormattedVector::new(*matrix)?;
            let mut iter = formatted.column_iter(index);
            let r_type = r_typeof(*matrix);
            let kind = if r_type == STRSXP {
                VariableKind::String
            } else if r_type == RAWSXP {
                VariableKind::Bytes
            } else if r_type == LGLSXP {
                VariableKind::Boolean
            } else {
                VariableKind::Number
            };

            for i in 0..n_row {
                out.push(Variable {
                    access_key: format!("{}", i),
                    display_name: format!("[{}, {}]", i + 1, index + 1),
                    display_value: iter.next().unwrap(),
                    display_type: String::from(""),
                    type_info: String::from(""),
                    kind: kind.clone(),
                    length: 1,
                    size: 0,
                    has_children: false,
                    is_truncated: false,
                    has_viewer: false,
                    updated_time: Self::update_timestamp(),
                });
            }

            Ok(out)
        }
    }

    fn inspect_vector(vector: SEXP) -> harp::error::Result<Vec<Variable>> {
        unsafe {
            let vector = RObject::new(vector);
            let n = Rf_xlength(*vector);

            let mut out: Vec<Variable> = vec![];
            let r_type = r_typeof(*vector);
            let formatted = FormattedVector::new(*vector)?;
            let names = Names::new(*vector, |i| format!("[{}]", i + 1));
            let kind = if r_type == STRSXP {
                VariableKind::String
            } else if r_type == RAWSXP {
                VariableKind::Bytes
            } else if r_type == LGLSXP {
                VariableKind::Boolean
            } else {
                VariableKind::Number
            };

            for i in 0..n {
                out.push(Variable {
                    access_key: format!("{}", i),
                    display_name: names.get_unchecked(i),
                    display_value: formatted.get_unchecked(i),
                    display_type: String::from(""),
                    type_info: String::from(""),
                    kind: kind.clone(),
                    length: 1,
                    size: 0,
                    has_children: false,
                    is_truncated: false,
                    has_viewer: false,
                    updated_time: Self::update_timestamp(),
                });
            }

            Ok(out)
        }
    }

    /// Creates an update timestamp for a variable
    fn update_timestamp() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    fn inspect_pairlist(value: SEXP) -> Result<Vec<Variable>, harp::error::Error> {
        let mut out: Vec<Variable> = vec![];

        let mut pairlist = value;
        unsafe {
            let mut i = 0;
            while pairlist != R_NilValue {
                r_assert_type(pairlist, &[LISTSXP])?;

                let tag = TAG(pairlist);
                let display_name = if r_is_null(tag) {
                    format!("[[{}]]", i + 1)
                } else {
                    String::from(RSymbol::new_unchecked(tag))
                };

                out.push(Self::from(i.to_string(), display_name, CAR(pairlist)).var());

                pairlist = CDR(pairlist);
                i = i + 1;
            }
        }

        Ok(out)
    }

    fn inspect_r6(value: RObject) -> Result<Vec<Variable>, harp::error::Error> {
        let mut has_private = false;
        let mut has_methods = false;

        let env = Environment::new(value);
        let mut childs: Vec<Variable> = env
            .iter()
            .filter_map(|b| b.ok())
            .filter(|b: &Binding| {
                if b.name == ".__enclos_env__" {
                    if let BindingValue::Standard { object, .. } = &b.value {
                        has_private =
                            Environment::new(RObject::view(object.sexp)).exists("private");
                    }

                    false
                } else if b.is_hidden() {
                    false
                } else {
                    match &b.value {
                        BindingValue::Standard { object, .. } |
                        BindingValue::Altrep { object, .. } => {
                            if r_typeof(object.sexp) == CLOSXP {
                                has_methods = true;
                                false
                            } else {
                                true
                            }
                        },

                        // active bindings and promises
                        _ => true,
                    }
                }
            })
            .map(|b| Self::new(&b).var())
            .collect();

        childs.sort_by(|a, b| a.display_name.cmp(&b.display_name));

        if has_private {
            childs.push(Variable {
                access_key: String::from("<private>"),
                display_name: String::from("private"),
                display_value: String::from("Private fields and methods"),
                display_type: String::from(""),
                type_info: String::from(""),
                kind: VariableKind::Other,
                length: 0,
                size: 0,
                has_children: true,
                is_truncated: false,
                has_viewer: false,
                updated_time: Self::update_timestamp(),
            });
        }

        if has_methods {
            childs.push(Variable {
                access_key: String::from("<methods>"),
                display_name: String::from("methods"),
                display_value: String::from("Methods"),
                display_type: String::from(""),
                type_info: String::from(""),
                kind: VariableKind::Other,
                length: 0,
                size: 0,
                has_children: true,
                is_truncated: false,
                has_viewer: false,
                updated_time: Self::update_timestamp(),
            });
        }

        Ok(childs)
    }

    fn inspect_environment(value: RObject) -> Result<Vec<Variable>, harp::error::Error> {
        let mut out: Vec<Variable> =
            Environment::new_filtered(value, EnvironmentFilter::ExcludeHidden)
                .iter()
                .filter_map(|b| b.ok())
                .map(|b| Self::new(&b).var())
                .collect();

        out.sort_by(|a, b| a.display_name.cmp(&b.display_name));

        Ok(out)
    }

    fn inspect_s4(value: SEXP) -> Result<Vec<Variable>, harp::error::Error> {
        let mut out: Vec<Variable> = vec![];

        unsafe {
            let slot_names = RFunction::new("methods", ".slotNames").add(value).call()?;

            let slot_names = CharacterVector::new_unchecked(*slot_names);
            let mut iter = slot_names.iter();
            while let Some(Some(display_name)) = iter.next() {
                let slot_symbol = r_symbol!(display_name);
                let slot: RObject = harp::try_catch(|| R_do_slot(value, slot_symbol).into())?;
                let access_key = display_name.clone();
                out.push(PositronVariable::from(access_key, display_name, slot.sexp).var());
            }
        }

        Ok(out)
    }

    fn inspect_r6_methods(value: RObject) -> Result<Vec<Variable>, harp::error::Error> {
        let mut out: Vec<Variable> = Environment::new(value)
            .iter()
            .filter_map(|b| b.ok())
            .filter(|b: &Binding| match &b.value {
                BindingValue::Standard { object, .. } => r_typeof(object.sexp) == CLOSXP,

                _ => false,
            })
            .map(|b| Self::new(&b).var())
            .collect();

        out.sort_by(|a, b| a.display_name.cmp(&b.display_name));

        Ok(out)
    }

    fn try_inspect_custom_method(value: SEXP) -> Result<Option<Vec<Variable>>, harp::Error> {
        let result: Option<RObject> = ArkGenerics::VariableGetChildren
            .try_dispatch(value, vec![])
            .map_err(|err| harp::Error::Anyhow(err))?;

        match result {
            None => Ok(None),
            Some(value) => {
                // Make sure value is a list before using inspect_list
                if !r_typeof(value.sexp) == LISTSXP {
                    return Err(harp::Error::Anyhow(anyhow!(
                        "Expected `{}` to return a list.",
                        ArkGenerics::VariableGetChildren.to_string()
                    )));
                }

                // This is essentially the same as Self::inspect_list but with modified `access_key`
                // that adds more information about the object:
                // 1. Provide the name and the index for the `get_child_at` method.
                // 2. (Not necessary) Given an access key, we can detect if we want to apply a custom get_child_method.
                let list = List::new(value.sexp)?;
                let n = unsafe { list.len() };

                let names = local!({
                    let r_names = unsafe { RObject::new(Rf_getAttrib(value.sexp, R_NamesSymbol)) };
                    if r_is_null(r_names.sexp) {
                        return vec![None; n];
                    }

                    let names = unsafe { CharacterVector::new_unchecked(r_names) };

                    if unsafe { names.len() } != n {
                        return vec![None; n];
                    }

                    names
                        .iter()
                        .map(|v| match v {
                            None => None,
                            Some(s) => {
                                if s.len() == 0 {
                                    None
                                } else {
                                    Some(s)
                                }
                            },
                        })
                        .collect()
                });

                let variables = list
                    .iter()
                    .zip(names.iter())
                    .enumerate()
                    .map(|(i, (x, name))| {
                        // The acess key is formatted as `custom-{index}-{length(name)}-{name}`
                        // where:
                        // - index: is the position of the element in children's list
                        // - length(name): the original length of the name, before truncation.
                        // - name: a possibly truncated name. Very large names could cause problems
                        //   when transfered to the UI.
                        let (access_name, name_len) = match name {
                            Some(nm) => {
                                let truncated_name: String =
                                    nm.chars().take(MAX_DISPLAY_VALUE_LENGTH).collect();
                                (truncated_name, nm.len())
                            },
                            None => (String::from(""), 0),
                        };

                        let access_key = format!("custom-{i}-{name_len}-{access_name}");

                        let display_name = name.clone().unwrap_or(format!("[[{}]]", i + 1));
                        Self::from(access_key, display_name, x).var()
                    })
                    .collect();

                Ok(Some(variables))
            },
        }
    }
}

fn try_from_method_variable_kind(value: SEXP) -> anyhow::Result<Option<VariableKind>> {
    let kind: Option<String> = ArkGenerics::VariableKind.try_dispatch(value, vec![])?;
    match kind {
        None => Ok(None),
        // We want to parse a VariableKind from it's string representation.
        // We do that by reading from a json which is just `"{kind}"`.
        Some(kind) => Ok(serde_json::from_str(format!(r#""{kind}""#).as_str())?),
    }
}

pub fn is_binding_fancy(binding: &Binding) -> bool {
    match &binding.value {
        BindingValue::Active { .. } => true,
        BindingValue::Altrep { .. } => true,
        _ => false,
    }
}

pub fn plain_binding_force_with_rollback(binding: &Binding) -> anyhow::Result<RObject> {
    match &binding.value {
        BindingValue::Standard { object, .. } => Ok(object.clone()),
        BindingValue::Promise { promise, .. } => Ok(r_promise_force_with_rollback(promise.sexp)?),
        _ => Err(anyhow!("Unexpected binding type")),
    }
}

fn parse_index(x: &String) -> harp::Result<isize> {
    x.parse::<isize>().map_err(|err| {
        harp::Error::Anyhow(anyhow!("Expected to be able to parse into integer: {err}"))
    })
}

#[cfg(test)]
mod tests {
    use harp;

    use super::*;
    use crate::r_task;

    #[test]
    fn test_variable_with_methods() {
        r_task(|| {
            // Register the display value method
            harp::parse_eval_global(
                r#"
                .ark.register_method("ark_positron_variable_display_value", "foo", function(x, width) {
                    # We return a large string and make sure it gets truncated.
                    paste0(rep("a", length.out = 2*width), collapse="")
                })

                .ark.register_method("ark_positron_variable_display_type", "foo", function(x, include_length) {
                    paste0("foo (", length(x), ")")
                })

                .ark.register_method("ark_positron_variable_has_children", "foo", function(x) {
                    TRUE
                })

                .ark.register_method("ark_positron_variable_kind", "foo", function(x) {
                    "other"
                })

                .ark.register_method("ark_positron_variable_get_children", "foo", function(x) {
                    children <- list(
                        "hello" = list(a = 1, b = 2),
                        "bye" = "testing",
                        c(1, 2, 3),
                        c(1, 2, 3, 4)
                    )
                    # Make a very large name to test truncation
                    names(children)[4] <- paste0(rep(letters, 100), collapse = "")
                    children
                })

                .ark.register_method("ark_positron_variable_get_child_at", "foo", function(x, ..., index, name) {
                    if (!is.null(name) && name == "hello") {
                        list(a = 1, b = 2)
                    } else if (index == 2) {
                        "testing"
                    } else if (index == 3) {
                        c(1, 2, 3)
                    } else if (index == 4) {
                        # The fourth element name is very large, so it should
                        # be discarded by ark.
                        if (!is.null(name)) {
                            stop("Name should have been discarded")
                        }
                        c(1, 2, 3, 4)
                    } else {
                        stop("Unexpected")
                    }
                })
                "#,
            )
            .unwrap();

            // Create an object with that class in an env.
            let env = harp::parse_eval_base(
                r#"
            local({
                env <- new.env(parent = emptyenv())
                env$x <- structure(list(1,2,3), class = "foo")
                env
            })
            "#,
            )
            .unwrap();

            let path = vec![];
            let variables = PositronVariable::inspect(env.clone(), &path).unwrap();

            assert_eq!(variables.len(), 1);
            let variable = variables[0].clone();

            assert_eq!(variable.display_value, "a".repeat(MAX_DISPLAY_VALUE_LENGTH));

            assert_eq!(variable.display_type, String::from("foo (3)"));

            assert_eq!(variable.has_children, true);

            assert_eq!(variable.kind, VariableKind::Other);

            // Now inspect `x`
            let path = vec![String::from("x")];
            let variables = PositronVariable::inspect(env.clone(), &path).unwrap();

            assert_eq!(variables.len(), 4);

            // Now inspect a list inside x
            let path = vec![String::from("x"), variables[0].access_key.clone()];
            let list = PositronVariable::inspect(env.clone(), &path).unwrap();
            assert_eq!(list.len(), 2);

            let path = vec![String::from("x"), variables[2].access_key.clone()];
            let vector = PositronVariable::inspect(env.clone(), &path).unwrap();
            assert_eq!(vector.len(), 3);

            let path = vec![String::from("x"), variables[3].access_key.clone()];
            let vector = PositronVariable::inspect(env, &path).unwrap();
            assert_eq!(vector.len(), 4);
        })
    }

    #[test]
    fn test_inspect_r6() {
        r_task(|| {
            // Skip test if R6 is not installed
            if let Ok(false) = harp::parse_eval_global(r#".ps.is_installed("R6")"#)
                .unwrap()
                .try_into()
            {
                return;
            }

            // Create an environment that contains an R6 class and an instance
            let env = harp::parse_eval_global("new.env()").unwrap();

            harp::parse_eval0(
                r#"
            Person <- R6::R6Class("Person",
                public = list(
                    name = NULL,
                    friend = NULL,
                    initialize = function(name = NA, friend = NA) {
                        self$name <- name
                        self$friend <- friend
                    },
                    greet = function() {
                        cat(paste0("Hello, my name is ", self$name, ".\n"))
                    }
                ),
                private = list(
                    get_friend = function() {
                        self$friend
                    }
                ),
                active = list(
                    active_name = function() {
                        stop("Variables pane should not evaluate active bindings.")
                    }
                )
            )

            x = Person$new("ann", NA)
            "#,
                env.clone(),
            )
            .unwrap();

            // Inspect the class instance
            let path = vec![String::from("x")];
            let fields = PositronVariable::inspect(env.clone(), &path).unwrap();

            // Is the active binding correctly handled?
            assert_eq!(fields.len(), 5);
            let n_active_bindings = fields
                .iter()
                .filter(|v| v.display_name.eq("active_name"))
                .map(|v| {
                    assert_eq!(v.display_value, "");
                    assert_eq!(v.display_type, "active binding");
                })
                .count();
            assert_eq!(n_active_bindings, 1);

            // Can we inspect the list of methods?
            let path = vec![String::from("x"), String::from("<methods>")];
            let fields = PositronVariable::inspect(env.clone(), &path).unwrap();
            assert_eq!(fields.len(), 3);
            let names: Vec<String> = fields.iter().map(|v| v.display_name.clone()).collect();
            assert_eq!(names, vec![
                String::from("clone"),
                String::from("greet"),
                String::from("initialize")
            ]);

            // Can we get a list of private methods?
            let path = vec![String::from("x"), String::from("<private>")];
            let fields = PositronVariable::inspect(env.clone(), &path).unwrap();
            assert_eq!(fields.len(), 1);
            let names: Vec<String> = fields.iter().map(|v| v.display_name.clone()).collect();
            assert_eq!(names, vec![String::from("get_friend"),]);
        })
    }

    #[test]
    fn test_inspect_list() {
        r_task(|| {
            // Create an environment that contains an R6 class and an instance
            let env = harp::parse_eval_global("new.env()").unwrap();

            harp::parse_eval0(
                r#"
                x <- list(
                    a = 123,
                    b = list(1,2,3),
                    1,
                    list(1,2,3)
                )
            "#,
                env.clone(),
            )
            .unwrap();

            // Inspect the list
            let path = vec![String::from("x")];
            let fields = PositronVariable::inspect(env.clone(), &path).unwrap();

            assert_eq!(fields.len(), 4);

            // Make sure we can see something with display_name a
            assert_eq!(fields.iter().filter(|v| v.display_name.eq("a")).count(), 1);

            // Check that the display value is correct for `a`
            assert_eq!(fields[0].display_value, "123");

            // Make sure empty named are formatted
            assert_eq!(
                fields.iter().filter(|v| v.display_name.eq("[[3]]")).count(),
                1
            );

            // Can we inspect list internals
            let path = vec![String::from("x"), String::from("1")];
            let fields = PositronVariable::inspect(env.clone(), &path).unwrap();

            assert_eq!(fields.len(), 3);
            fields.iter().enumerate().for_each(|(index, value)| {
                let index = index + 1; // R indexes start from 1
                assert_eq!(value.display_name, format!("[[{index}]]"));
            });
        })
    }

    #[test]
    fn test_inspect_s4() {
        r_task(|| {
            let env = harp::parse_eval_global("new.env()").unwrap();

            harp::parse_eval0(
                r#"
                setClass("Person", representation(name = "character", age = "numeric", objects = "list"))
                x <- new("Person", name = "x", age = 31, objects = list(1,2,3))
            "#,
                env.clone(),
            )
            .unwrap();

            // Inspect the S4 object
            let path = vec![String::from("x")];
            let fields = PositronVariable::inspect(env.clone(), &path).unwrap();

            assert_eq!(fields.len(), 3);

            // Can we inspect `objects`?
            let path = vec![String::from("x"), String::from("objects")];
            let fields = PositronVariable::inspect(env.clone(), &path).unwrap();

            assert_eq!(fields.len(), 3);
            fields.iter().enumerate().for_each(|(index, value)| {
                let index = index + 1; // R indexes start from 1
                assert_eq!(value.display_name, format!("[[{index}]]"));
            });
        })
    }
}
