use itertools::Itertools;
use libr::*;
use stdext::unwrap;

use crate::environment::Environment;
use crate::environment::EnvironmentFilter;
use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::object::r_length;
use crate::object::r_list_get;
use crate::object::RObject;
use crate::utils::pairlist_size;
use crate::utils::r_altrep_class;
use crate::utils::r_classes;
use crate::utils::r_inherits;
use crate::utils::r_is_altrep;
use crate::utils::r_is_data_frame;
use crate::utils::r_is_matrix;
use crate::utils::r_is_null;
use crate::utils::r_is_s4;
use crate::utils::r_is_simple_vector;
use crate::utils::r_typeof;
use crate::utils::r_vec_is_single_dimension_with_single_value;
use crate::utils::r_vec_shape;
use crate::utils::r_vec_type;
use crate::vector::formatted_vector::FormattedVector;
use crate::vector::names::Names;
use crate::vector::CharacterVector;
use crate::vector::IntegerVector;
use crate::vector::Vector;

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
        let dim = harp::df_dim(value);
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
            let display_i = Self::from(r_list_get(value, i));
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

        display_value.push_str("]");
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
        let environment = Environment::new(RObject::view(value));
        let environment_length = environment.length(EnvironmentFilter::ExcludeHiddenBindings);

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

        display_value.push_str("{");

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

        display_value.push_str("}");

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
            display_value.push_str("[");
            for i in 0..n_col {
                if first {
                    first = false;
                } else {
                    display_value.push_str(", ");
                }

                display_value.push_str("[");
                let display_column = formatted.column_iter(i).join(" ");
                if display_column.len() > MAX_DISPLAY_VALUE_LENGTH {
                    is_truncated = true;
                    // TODO: maybe this should only push_str() a slice
                    //       of the first n (MAX_WIDTH?) characters in that case ?
                }
                display_value.push_str(display_column.as_str());
                display_value.push_str("]");

                if display_value.len() > MAX_DISPLAY_VALUE_LENGTH {
                    is_truncated = true;
                }
                if is_truncated {
                    break;
                }
            }
            display_value.push_str("]");
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
                display_value.push_str(" ");
            }
            display_value.push_str(&x);
            if display_value.len() > MAX_DISPLAY_VALUE_LENGTH {
                is_truncated = true;
                break;
            }
        }

        Self::new(display_value, is_truncated)
    }

    fn from_error(err: crate::Error) -> Self {
        log::warn!("Error while formatting variable: {err:?}");
        Self::new(String::from("??"), true)
    }
}

pub struct WorkspaceVariableDisplayType {
    pub display_type: String,
    pub type_info: String,
}

impl WorkspaceVariableDisplayType {
    pub fn from(value: SEXP) -> Self {
        if r_is_null(value) {
            return Self::simple(String::from("NULL"));
        }

        if r_is_s4(value) {
            return Self::from_class(value, String::from("S4"));
        }

        if r_is_simple_vector(value) {
            let display_type: String;
            if r_vec_is_single_dimension_with_single_value(value) {
                display_type = r_vec_type(value);
            } else {
                display_type = format!("{} [{}]", r_vec_type(value), r_vec_shape(value));
            }

            let mut type_info = display_type.clone();
            if r_is_altrep(value) {
                type_info.push_str(r_altrep_class(value).as_str())
            }

            return Self::new(display_type, type_info);
        }

        let rtype = r_typeof(value);
        match rtype {
            EXPRSXP => {
                let default = format!("expression [{}]", unsafe { Rf_xlength(value) });
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

            LISTSXP => match pairlist_size(value) {
                Ok(n) => Self::simple(format!("pairlist [{}]", n)),
                Err(_) => Self::simple(String::from("pairlist [?]")),
            },

            VECSXP => unsafe {
                if r_is_data_frame(value) {
                    let classes = r_classes(value).unwrap();
                    let dfclass = classes.get_unchecked(0).unwrap();

                    let dim = RFunction::new("base", "dim.data.frame")
                        .add(value)
                        .call()
                        .unwrap();
                    let shape = FormattedVector::new(*dim).unwrap().iter().join(", ");
                    let display_type = format!("{} [{}]", dfclass, shape);
                    Self::simple(display_type)
                } else {
                    let default = format!("list [{}]", Rf_xlength(value));
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

    fn new(display_type: String, type_info: String) -> Self {
        Self {
            display_type,
            type_info,
        }
    }
}
