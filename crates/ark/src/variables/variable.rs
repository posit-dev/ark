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
use harp::table_info;
use harp::utils::pairlist_size;
use harp::utils::r_altrep_class;
use harp::utils::r_assert_type;
use harp::utils::r_classes;
use harp::utils::r_format_s4;
use harp::utils::r_inherits;
use harp::utils::r_is_altrep;
use harp::utils::r_is_data_frame;
use harp::utils::r_is_matrix;
use harp::utils::r_is_null;
use harp::utils::r_is_s4;
use harp::utils::r_is_simple_vector;
use harp::utils::r_is_unbound;
use harp::utils::r_promise_force_with_rollback;
use harp::utils::r_type2char;
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
use crate::modules::ARK_ENVS;

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
        Self::try_from(value).unwrap_or_else(|err| Self::from_error(harp::Error::Anyhow(err)))
    }

    pub fn try_from(value: SEXP) -> anyhow::Result<Self> {
        // Try to use the display method if there's one available\
        if let Some(display_value) = Self::try_from_method(value) {
            return Ok(display_value);
        }

        let out = match r_typeof(value) {
            NILSXP => Self::new(String::from("NULL"), false),
            VECSXP if r_inherits(value, "data.frame") => Self::from_data_frame(value),
            VECSXP if !r_inherits(value, "POSIXlt") => Self::from_list(value),
            LISTSXP => Self::empty(),
            SYMSXP if value == unsafe { R_MissingArg } => {
                Self::new(String::from("<missing>"), false)
            },
            CLOSXP => Self::from_closure(value),
            ENVSXP => Self::from_env(value),
            LANGSXP => Self::from_language(value),
            CHARSXP => Self::from_charsxp(value),
            _ if r_is_matrix(value) => Self::from_matrix(value)?,
            RAWSXP | LGLSXP | INTSXP | REALSXP | STRSXP | CPLXSXP => Self::from_default(value)?,
            _ if r_is_s4(value) => Self::from_s4(value)?,
            _ => Self::from_error(Error::Anyhow(anyhow!(
                "Unexpected type {}",
                r_type2char(r_typeof(value))
            ))),
        };

        Ok(out)
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

    fn from_language(value: SEXP) -> Self {
        if r_inherits(value, "formula") {
            return match Self::from_formula(value) {
                Ok(display_value) => display_value,
                Err(err) => Self::from_error(harp::Error::Anyhow(err)),
            };
        }

        return Self::from_error(Error::Anyhow(anyhow!("Unexpected language object type")));
    }

    fn from_formula(value: SEXP) -> anyhow::Result<Self> {
        // `format` for formula will return a character vector, splitting the expressions within
        // the formula, for instance `~{x + y}` will be split into `~` and `["~{", "x", "}"]`.
        let formatted: Vec<String> = RFunction::new("base", "format")
            .add(value)
            .call_in(ARK_ENVS.positron_ns)?
            .try_into()?;

        if formatted.len() < 1 {
            return Err(anyhow!("Failed to format formula"));
        }

        let (mut truncated, mut display_value) =
            truncate_chars(formatted[0].clone(), MAX_DISPLAY_VALUE_LENGTH);

        if formatted.len() > 1 {
            display_value.push_str(" ...");
            truncated = true;
        }

        Ok(Self::new(display_value, truncated))
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
                break;
            }
        }

        if !is_truncated {
            display_value.push_str("]");
        }

        Self::new(display_value, is_truncated)
    }

    fn from_closure(value: SEXP) -> Self {
        unsafe {
            let args = RFunction::from("args").add(value).call().unwrap();
            let formatted = RFunction::from("format").add(args.sexp).call().unwrap();
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
    fn from_matrix(value: SEXP) -> anyhow::Result<Self> {
        let formatted = FormattedVector::new(RObject::from(value))?;

        let mut display_value = String::from("");

        let n_col = match harp::table_info(value) {
            Some(info) => info.dims.num_cols,
            None => {
                log::error!("Failed to get matrix dimensions");
                0
            },
        };

        display_value.push('[');
        for i in 0..n_col {
            if i > 0 {
                display_value.push_str(", ");
            }

            display_value.push('[');
            for char in formatted
                .column_iter_n(i as isize, MAX_DISPLAY_VALUE_LENGTH)?
                .join(" ")
                .chars()
            {
                if display_value.len() >= MAX_DISPLAY_VALUE_LENGTH {
                    return Ok(Self::new(display_value, true));
                }
                display_value.push(char);
            }
            display_value.push(']');
        }
        display_value.push(']');

        Ok(Self::new(display_value, false))
    }

    fn from_s4(value: SEXP) -> anyhow::Result<Self> {
        let result: Vec<String> = RObject::from(r_format_s4(value)?).try_into()?;
        let mut display_value = String::from("");
        for val in result.iter() {
            for char in val.chars() {
                if display_value.len() >= MAX_DISPLAY_VALUE_LENGTH {
                    return Ok(Self::new(display_value, true));
                }
                display_value.push(char);
            }
        }
        Ok(Self::new(display_value, false))
    }

    fn from_charsxp(_: SEXP) -> Self {
        Self::new(String::from("<CHARSXP>"), false)
    }

    fn from_default(value: SEXP) -> anyhow::Result<Self> {
        let formatted = FormattedVector::new(RObject::from(value))?;

        let mut display_value = String::with_capacity(MAX_DISPLAY_VALUE_LENGTH);
        let mut is_truncated = false;

        // Performance: value is potentially a very large vector, so we need to be careful
        // to not format every element of value. Instead only format the necessary elements
        // to display the first MAX_DISPLAY_VALUE_LENGTH characters.
        'outer: for (i, elt) in formatted.iter_take(MAX_DISPLAY_VALUE_LENGTH)?.enumerate() {
            if i > 0 {
                display_value.push_str(" ");
            }
            for char in elt.chars() {
                if display_value.len() >= MAX_DISPLAY_VALUE_LENGTH {
                    is_truncated = true;
                    break 'outer;
                }
                display_value.push(char);
            }
        }

        Ok(Self::new(display_value, is_truncated))
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

        // We can't check attributes of CHARSXP, so we just short-circuit here
        if r_typeof(value) == CHARSXP {
            return Self::simple(String::from("CHARSXP"));
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
                            let dim = table_info(value);
                            let shape = match dim {
                                Some(info) => {
                                    format!("{}, {}", info.dims.num_rows, info.dims.num_cols)
                                },
                                None => String::from("?, ?"),
                            };
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
                let mut variable = Self::from(display_name.clone(), display_name, object.sexp);

                let size = match object.size() {
                    Ok(size) => size as i64,
                    Err(err) => {
                        log::warn!("Can't compute size of object: {err}");
                        0
                    },
                };

                variable.var.size = size;
                variable
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

        Self {
            var: Variable {
                access_key,
                display_name,
                display_value,
                display_type,
                type_info,
                kind,
                length: Self::variable_length(x) as i64,
                size: 0, // It's up to the caller to set the size.
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

    pub fn inspect(env: RObject, path: &Vec<String>) -> anyhow::Result<Vec<Variable>> {
        let node = Self::resolve_object_from_path(env, &path)?;

        match node {
            EnvironmentVariableNode::R6Node { object, name } => match name.as_str() {
                "<private>" => {
                    let env = Environment::new(object);
                    let enclos = Environment::new(RObject::new(env.find(".__enclos_env__")?));
                    let private = RObject::new(enclos.find("private")?);

                    Ok(Self::inspect_environment(private)?)
                },

                "<methods>" => Ok(Self::inspect_r6_methods(object)?),

                _ => Err(anyhow!("Unexpected path {:?}", path)),
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
                    Ok(Self::inspect_s4(object.sexp)?)
                } else {
                    match r_typeof(object.sexp) {
                        VECSXP | EXPRSXP => Ok(Self::inspect_list(object.sexp)?),
                        LISTSXP => Ok(Self::inspect_pairlist(object.sexp)?),
                        ENVSXP => {
                            if r_inherits(object.sexp, "R6") {
                                Ok(Self::inspect_r6(object)?)
                            } else {
                                Ok(Self::inspect_environment(object)?)
                            }
                        },
                        LGLSXP | RAWSXP | STRSXP | INTSXP | REALSXP | CPLXSXP => {
                            if r_is_matrix(object.sexp) {
                                Self::inspect_matrix(object.sexp)
                            } else {
                                Ok(Self::inspect_vector(object.sexp)?)
                            }
                        },
                        _ => Ok(vec![]),
                    }
                }
            },

            EnvironmentVariableNode::Matrixcolumn { object, index } => {
                Ok(Self::inspect_matrix_column(object.sexp, index)?)
            },
            EnvironmentVariableNode::AtomicVectorElement { .. } => Ok(vec![]),
        }
    }

    pub fn clip(
        env: RObject,
        path: &Vec<String>,
        _format: &ClipboardFormatFormat,
    ) -> anyhow::Result<String> {
        let node = Self::resolve_object_from_path(env, &path)?;

        match node {
            EnvironmentVariableNode::Concrete { object } => {
                if r_is_data_frame(object.sexp) {
                    let formatted = RFunction::from(".ps.environment.clipboardFormatDataFrame")
                        .add(object)
                        .call()?;

                    Ok(FormattedVector::new(formatted)?.iter()?.join("\n"))
                } else if r_typeof(object.sexp) == CLOSXP {
                    let deparsed: Vec<String> = RFunction::from("deparse")
                        .add(object.sexp)
                        .call()?
                        .try_into()?;

                    Ok(deparsed.join("\n"))
                } else {
                    Ok(FormattedVector::new(object)?.iter()?.join(" "))
                }
            },
            EnvironmentVariableNode::R6Node { .. } => Ok(String::from("")),
            EnvironmentVariableNode::AtomicVectorElement { object, index } => {
                let formatted = FormattedVector::new(object)?;
                Ok(formatted.format_elt(index)?)
            },
            EnvironmentVariableNode::Matrixcolumn { object, index } => {
                let clipped = FormattedVector::new(object)?.column_iter(index)?.join(" ");
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
        let mut x = unsafe { Rf_findVarInFrame(object.sexp, symbol) };

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
        match parse_custom_access_key(access_key) {
            Ok(None) => {}, // Do nothing and proceed,
            Ok(Some((name, index))) => {
                let node =
                    ArkGenerics::VariableGetChildAt.try_dispatch::<RObject>(object.sexp, vec![
                        RArgument::new("index", RObject::from(index + 1)), // Index is 0-based, so we convert to 1-based for R.
                        RArgument::new("name", RObject::from(name)),
                    ]);
                match node {
                    Ok(None) => {
                        // The object doesn't have a custom get_child_at method. We continue to built-in methods.
                    },
                    Ok(Some(child)) => {
                        return Ok(EnvironmentVariableNode::Concrete { object: child });
                    },
                    Err(err) => {
                        // It's not safe to apply default methods in this case, because we rely on custom
                        // access keys, which could indicate the access index depending on the node implementation.
                        // See for instance, how it's used to index lists and atomic vectors.
                        return Err(harp::Error::Anyhow(err));
                    },
                }
            },
            Err(err) => {
                return Err(harp::Error::Anyhow(err));
            },
        }

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

        match r_typeof(object.sexp) {
            ENVSXP => Self::get_envsxp_child_node_at(object, access_key),
            VECSXP | EXPRSXP => {
                let index = parse_index(access_key)?;
                Ok(EnvironmentVariableNode::Concrete {
                    object: RObject::view(harp::list_get(object.sexp, index)),
                })
            },
            LISTSXP => {
                let mut pairlist = object.sexp;
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
                let dim = IntegerVector::new(Rf_getAttrib(object.sexp, R_DimSymbol))?;
                let n_row = dim.get_unchecked(0).unwrap() as isize;
                let row_index = parse_index(path_elt)?;

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
        let list = List::new(value)?;
        let names = Names::new(value, |i| format!("[[{}]]", i + 1));

        let variables: Vec<Variable> = list
            .iter()
            .enumerate()
            .take(MAX_DISPLAY_VALUE_ENTRIES)
            .map(|(i, value)| {
                let (_, display_name) =
                    truncate_chars(names.get_unchecked(i as isize), MAX_DISPLAY_VALUE_LENGTH);
                Self::from(i.to_string(), display_name, value).var()
            })
            .collect();

        Ok(variables)
    }

    fn inspect_matrix(matrix: SEXP) -> anyhow::Result<Vec<Variable>> {
        let matrix = RObject::new(matrix);

        let n_col = match harp::table_info(matrix.sexp) {
            Some(info) => info.dims.num_cols,
            None => {
                log::warn!("Unexpected matrix object. Couldn't get dimensions.");
                return Ok(vec![]);
            },
        };

        let make_variable = |access_key, display_name, display_value, is_truncated| Variable {
            access_key,
            display_name,
            display_value,
            display_type: String::from(""),
            type_info: String::from(""),
            kind: VariableKind::Collection,
            length: 1,
            size: 0,
            has_children: true,
            is_truncated,
            has_viewer: false,
            updated_time: Self::update_timestamp(),
        };

        let formatted = FormattedVector::new(matrix)?;
        let mut variables = Vec::with_capacity(n_col as usize);

        for col in (0..n_col).take(MAX_DISPLAY_VALUE_ENTRIES) {
            // The display value of columns concatenates the column vector values into a
            // single string with maximum length of MAX_DISPLAY_VALUE_LENGTH.
            let mut is_truncated = false;
            let mut display_value = String::with_capacity(MAX_DISPLAY_VALUE_LENGTH);

            let iter = formatted
                // Even if each column element takes 0 characters, `MAX_DISPLAY_VALUE_LENGTH`
                // is enough to fill the display value because we need to account for the space
                // between elements.
                .column_iter_n(col as isize, MAX_DISPLAY_VALUE_LENGTH)?
                .enumerate();

            'outer: for (i, elt) in iter {
                if i > 0 {
                    display_value.push_str(" ");
                }
                for char in elt.chars() {
                    if display_value.len() >= MAX_DISPLAY_VALUE_LENGTH {
                        is_truncated = true;
                        // We break the outer loop to avoid adding more characters to the
                        // display value.
                        break 'outer;
                    }
                    display_value.push(char);
                }
            }

            variables.push(make_variable(
                format!("{}", col),
                format!("[, {}]", col + 1),
                display_value,
                is_truncated,
            ));
        }

        Ok(variables)
    }

    fn inspect_matrix_column(matrix: SEXP, index: isize) -> anyhow::Result<Vec<Variable>> {
        let column = harp::table::tbl_get_column(matrix, index as i32, harp::TableKind::Matrix)?;

        let variables: Vec<Variable> = Self::inspect_vector(column.sexp)?
            .into_iter()
            .enumerate()
            .map(|(row, mut var)| {
                var.display_name = format!("[{}, {}]", row + 1, index + 1);
                var
            })
            .collect();

        Ok(variables)
    }

    fn inspect_vector(vector: SEXP) -> anyhow::Result<Vec<Variable>> {
        let vector = RObject::new(vector);

        let r_type = r_typeof(vector.sexp);
        let kind = match r_type {
            STRSXP => VariableKind::String,
            RAWSXP => VariableKind::Bytes,
            LGLSXP => VariableKind::Boolean,
            INTSXP | REALSXP | CPLXSXP => VariableKind::Number,
            _ => {
                log::warn!("Unexpected vector type: {r_type}");
                VariableKind::Other
            },
        };

        let make_variable =
            |access_key, display_name, display_value, kind, is_truncated| Variable {
                access_key,
                display_name,
                display_value,
                display_type: String::from(""),
                type_info: String::from(""),
                kind,
                length: 1,
                size: 0,
                has_children: false,
                is_truncated,
                has_viewer: false,
                updated_time: Self::update_timestamp(),
            };

        let formatted = FormattedVector::new(vector.clone())?;
        let names = Names::new(vector.sexp, |i| format!("[{}]", i + 1));

        let variables: Vec<Variable> = formatted
            .iter_take(MAX_DISPLAY_VALUE_ENTRIES)?
            .enumerate()
            .map(|(i, value)| {
                let (is_truncated, display_value) = truncate_chars(value, MAX_DISPLAY_VALUE_LENGTH);
                // Names are arbitrarily set by users, so we add a safeguard to truncate them
                // to avoid massive names that could break communications with the frontend.
                let (_, display_name) =
                    truncate_chars(names.get_unchecked(i as isize), MAX_DISPLAY_VALUE_LENGTH);

                make_variable(
                    format!("{}", i),
                    display_name,
                    display_value,
                    kind.clone(),
                    is_truncated,
                )
            })
            .collect();

        Ok(variables)
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
        Ok(out
            .get(0..std::cmp::min(out.len(), MAX_DISPLAY_VALUE_ENTRIES))
            .ok_or(Error::Anyhow(anyhow!("Unexpected environment size?")))?
            .to_vec())
    }

    fn inspect_s4(value: SEXP) -> Result<Vec<Variable>, harp::error::Error> {
        let mut out: Vec<Variable> = vec![];

        unsafe {
            let slot_names = RFunction::new("methods", ".slotNames").add(value).call()?;

            let slot_names = CharacterVector::new_unchecked(slot_names.sexp);
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

    fn try_inspect_custom_method(value: SEXP) -> anyhow::Result<Option<Vec<Variable>>> {
        let result: Option<RObject> = ArkGenerics::VariableGetChildren
            .try_dispatch(value, vec![])
            .map_err(|err| harp::Error::Anyhow(err))?;

        match result {
            None => Ok(None),
            Some(value) => {
                // Make sure value is a list before using inspect_list
                if !r_typeof(value.sexp) == LISTSXP {
                    return Err(anyhow!(
                        "Expected `{}` to return a list.",
                        ArkGenerics::VariableGetChildren.to_string()
                    ));
                }

                // This is essentially the same as Self::inspect_list but with modified `access_key`
                // that adds more information about the object:
                // 1. Provide the name and the index for the `get_child_at` method.
                // 2. (Not necessary) Given an access key, we can detect if we want to apply a custom get_child_method.
                let list = List::new(value.sexp)?;
                let n = unsafe { list.len() };

                let names = match value.names() {
                    None => vec![None; n],
                    Some(names) => names,
                };

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

fn parse_custom_access_key(access_key: &String) -> anyhow::Result<Option<(RObject, i32)>> {
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

    let name: RObject = match parsed_access_key[3] {
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

    Ok(Some((name, index)))
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

// We need to be careful when truncating the string, we don't want to return invalid
// UTF8 sequences. `chars` makes sure we are not splitting a UTF8 character in half.
// See also https://doc.rust-lang.org/book/ch08-02-strings.html#slicing-strings
fn truncate_chars(value: String, len: usize) -> (bool, String) {
    if value.len() > len {
        (true, value.chars().take(len).collect())
    } else {
        (false, value.clone())
    }
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

    fn inspect_from_expr(code: &str) -> Vec<Variable> {
        let env = Environment::new(harp::parse_eval_base("new.env(parent = emptyenv())").unwrap());
        let value = harp::parse_eval_base(code).unwrap();
        env.bind("x".into(), &value);
        // Inspect the S4 object
        let path = vec![String::from("x")];
        PositronVariable::inspect(env.into(), &path).unwrap()
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

            let path = vec![];
            let vars = PositronVariable::inspect(env.clone(), &path).unwrap();

            assert_eq!(vars.len(), 1);
            // Matching equality is not nice because the default `format` method for S4 objects
            // uses different quoting characters on Windows vs Unix.
            // Unix: <S4 class ‘ddiMatrix’ [package “Matrix”] with 4 slots>
            // Windows: <S4 class 'ddiMatrix' [package "Matrix"] with 4 slots>
            assert!(vars[0].display_value.starts_with("<S4 class"));

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

    #[test]
    fn test_truncation() {
        r_task(|| {
            let vars = inspect_from_expr("as.list(1:10000)");
            assert_eq!(vars.len(), MAX_DISPLAY_VALUE_ENTRIES);

            let vars = inspect_from_expr("1:10000");
            assert_eq!(vars.len(), MAX_DISPLAY_VALUE_ENTRIES);

            let vars = inspect_from_expr("rep(letters, length.out = 10000)");
            assert_eq!(vars.len(), MAX_DISPLAY_VALUE_ENTRIES);

            let vars = inspect_from_expr("matrix(0, ncol = 10000, nrow = 10000)");
            assert_eq!(vars.len(), MAX_DISPLAY_VALUE_ENTRIES);
            assert_eq!(vars[0].display_value.len(), MAX_DISPLAY_VALUE_LENGTH);
            assert_eq!(vars[0].is_truncated, true);

            let vars = inspect_from_expr("new.env(parent=emptyenv())");
            assert_eq!(vars.len(), 0);

            let vars = inspect_from_expr(
                "list2env(structure(as.list(1:10000), names = paste0('a', 1:10000)))",
            );
            assert_eq!(vars.len(), MAX_DISPLAY_VALUE_ENTRIES);
            assert_eq!(vars[0].display_name, "a1");

            let vars = inspect_from_expr(
                "rep(paste0(rep(letters, length.out = 10000), collapse = ''), 10)",
            );
            assert_eq!(vars.len(), 10);
            assert_eq!(vars[0].display_value.len(), MAX_DISPLAY_VALUE_LENGTH);
            assert_eq!(vars[0].is_truncated, true);

            let vars = inspect_from_expr(
                "structure(1:10, names = rep(paste(rep(letters, length.out = 10000), collapse = ''), 10))",
            );
            assert_eq!(vars[0].display_name.len(), MAX_DISPLAY_VALUE_LENGTH);

            let vars = inspect_from_expr(
                "structure(as.list(1:10), names = rep(paste(rep(letters, length.out = 10000), collapse = ''), 10))",
            );
            assert_eq!(vars[0].display_name.len(), MAX_DISPLAY_VALUE_LENGTH);
        })
    }

    #[test]
    fn test_support_formula() {
        r_task(|| {
            let vars = inspect_from_expr("list(x = x ~ y + z + a)");
            assert_eq!(vars[0].display_value, "x ~ y + z + a");

            let vars = inspect_from_expr("list(x = x ~ {y + z + a})");
            assert_eq!(vars[0].display_value, "x ~ { ...");
            assert_eq!(vars[0].is_truncated, true);

            let formula: String = (0..100).map(|i| format!("x{i}")).collect_vec().join(" + ");
            let vars = inspect_from_expr(format!("list(x = x ~ {formula})").as_str());

            assert_eq!(vars[0].is_truncated, true);
            // The deparser truncates the formula at 70 characters so we don't expect to get to
            // MAX_DISPLAY_VALUE_LENGTH. We do have protections if this behavior changes, though.
            assert_eq!(vars[0].display_value.len(), 70);
        })
    }

    #[test]
    fn test_truncation_on_matrices() {
        r_task(|| {
            let env = Environment::new_empty().unwrap();
            let value = harp::parse_eval_base("matrix(0, nrow = 10000, ncol = 10000)").unwrap();
            env.bind("x".into(), &value);

            // Inspect the matrix, we should see the list of columns truncated
            let path = vec![String::from("x")];
            let vars = PositronVariable::inspect(env.clone().into(), &path).unwrap();
            assert_eq!(vars.len(), MAX_DISPLAY_VALUE_ENTRIES);

            // Now inspect the first column
            let path = vec![String::from("x"), vars[0].access_key.clone()];
            let vars = PositronVariable::inspect(env.into(), &path).unwrap();
            assert_eq!(vars.len(), MAX_DISPLAY_VALUE_ENTRIES);
            assert_eq!(vars[0].display_name, "[1, 1]");
        });
    }

    #[test]
    fn test_string_truncation() {
        r_task(|| {
            let env = Environment::new_empty().unwrap();
            let value = harp::parse_eval_base("paste(1:5e6, collapse = ' - ')").unwrap();
            env.bind("x".into(), &value);

            let path = vec![];
            let vars = PositronVariable::inspect(env.into(), &path).unwrap();
            assert_eq!(vars.len(), 1);
            assert_eq!(vars[0].display_value.len(), MAX_DISPLAY_VALUE_LENGTH);
            assert_eq!(vars[0].is_truncated, true);

            // Test for the empty string
            let env = Environment::new_empty().unwrap();
            let value = harp::parse_eval_base("''").unwrap();
            env.bind("x".into(), &value);

            let path = vec![];
            let vars = PositronVariable::inspect(env.into(), &path).unwrap();
            assert_eq!(vars.len(), 1);
            assert_eq!(vars[0].display_value, "\"\"");

            // Test for the single elment matrix, but with a large character
            let env = Environment::new_empty().unwrap();
            let value = harp::parse_eval_base("matrix(paste(1:5e6, collapse = ' - '))").unwrap();
            env.bind("x".into(), &value);
            let path = vec![];
            let vars = PositronVariable::inspect(env.into(), &path).unwrap();
            assert_eq!(vars.len(), 1);
            assert_eq!(vars[0].display_value.len(), MAX_DISPLAY_VALUE_LENGTH);
            assert_eq!(vars[0].is_truncated, true);

            // Test for the empty matrix
            let env = Environment::new_empty().unwrap();
            let value = harp::parse_eval_base("matrix(NA, ncol = 0, nrow = 0)").unwrap();
            env.bind("x".into(), &value);
            let path = vec![];
            let vars = PositronVariable::inspect(env.into(), &path).unwrap();
            assert_eq!(vars.len(), 1);
            assert_eq!(vars[0].display_value, "[]");
        });
    }

    #[test]
    fn test_s4_with_different_length() {
        r_task(|| {
            let env = Environment::new_empty().unwrap();
            // Matrix::Matrix objects have length != 1, but their format() method returns a length 1 character
            // describing their class.
            let value = harp::parse_eval_base("Matrix::Matrix(0, nrow= 10, ncol = 10)").unwrap();
            env.bind("x".into(), &value);

            let path = vec![];
            let vars = PositronVariable::inspect(env.into(), &path).unwrap();
            assert_eq!(vars.len(), 1);
            assert!(vars[0].display_value.starts_with("<S4 class"),);
        })
    }

    #[test]
    fn test_charsxp() {
        r_task(|| {
            // Skip test if rlang is not installed
            if let Ok(false) = harp::parse_eval_global(r#".ps.is_installed("rlang")"#)
                .unwrap()
                .try_into()
            {
                return;
            }

            let env = Environment::new_empty().unwrap();
            let value = harp::parse_eval_base(r#"rlang:::chr_get("foo", 0L)"#).unwrap();
            env.bind("x".into(), &value);

            let path = vec![];
            let vars = PositronVariable::inspect(env.into(), &path).unwrap();
            assert_eq!(vars.len(), 1);
            assert_eq!(vars[0].display_value, "<CHARSXP>");
        })
    }
}
