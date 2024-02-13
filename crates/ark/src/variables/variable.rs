//
// variable.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::variables_comm::ClipboardFormatFormat;
use amalthea::comm::variables_comm::Variable;
use amalthea::comm::variables_comm::VariableKind;
use harp::call::RCall;
use harp::environment::Binding;
use harp::environment::BindingValue;
use harp::environment::Environment;
use harp::environment::EnvironmentFilter;
use harp::error::Error;
use harp::exec::r_try_catch;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::r_symbol;
use harp::symbol::RSymbol;
use harp::utils::pairlist_size;
use harp::utils::r_assert_type;
use harp::utils::r_inherits;
use harp::utils::r_is_data_frame;
use harp::utils::r_is_matrix;
use harp::utils::r_is_null;
use harp::utils::r_is_unbound;
use harp::utils::r_typeof;
use harp::variable::WorkspaceVariableDisplayType;
use harp::variable::WorkspaceVariableDisplayValue;
use harp::vector::formatted_vector::FormattedVector;
use harp::vector::names::Names;
use harp::vector::CharacterVector;
use harp::vector::IntegerVector;
use harp::vector::Vector;
use itertools::Itertools;
use libr::*;
use stdext::local;

fn has_children(value: SEXP) -> bool {
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
            ENVSXP => !Environment::new(RObject::view(value))
                .is_empty(EnvironmentFilter::ExcludeHiddenBindings),
            LGLSXP | RAWSXP | STRSXP | INTSXP | REALSXP | CPLXSXP => unsafe {
                Rf_xlength(value) > 1
            },
            _ => false,
        }
    }
}

enum EnvironmentVariableNode {
    Concrete { object: RObject },
    Artificial { object: RObject, name: String },
    Matrixcolumn { object: RObject, index: isize },
    VectorElement { object: RObject, index: isize },
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
        } = WorkspaceVariableDisplayType::from(x);

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
                size: RObject::view(x).size() as i64,
                has_children: has_children(x),
                is_truncated,
                has_viewer: r_is_data_frame(x) || r_is_matrix(x),
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
                        let code = RCall::new(code)?;
                        let fun = RSymbol::new(CAR(*code))?;
                        if fun == "lazyLoadDBfetch" {
                            return Ok(String::from("(unevaluated)"))
                        }

                        RFunction::from(".ps.environment.describeCall")
                            .add(code)
                            .call()?
                            .try_into()
                    },
                    _ => Err(Error::UnexpectedType(r_typeof(code), vec!(SYMSXP, LANGSXP)))
                }
            }
        };

        Self {
            var: Variable {
                access_key: display_name.clone(),
                display_name,
                display_value: display_value.unwrap_or(String::from("(unevaluated)")),
                display_type: String::from("promise"),
                type_info: String::from("promise"),
                kind: VariableKind::Lazy,
                length: 0,
                size: 0,
                has_children: false,
                is_truncated: false,
                has_viewer: false,
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
            },
        }
    }

    fn variable_length(x: SEXP) -> usize {
        let rtype = r_typeof(x);
        match rtype {
            LGLSXP | RAWSXP | INTSXP | REALSXP | CPLXSXP | STRSXP => unsafe {
                Rf_xlength(x) as usize
            },
            VECSXP => unsafe {
                if r_inherits(x, "POSIXlt") {
                    Rf_xlength(VECTOR_ELT(x, 0)) as usize
                } else if r_is_data_frame(x) {
                    let dim = RFunction::new("base", "dim.data.frame")
                        .add(x)
                        .call()
                        .unwrap();

                    INTEGER_ELT(*dim, 0) as usize
                } else {
                    Rf_xlength(x) as usize
                }
            },
            LISTSXP => match pairlist_size(x) {
                Ok(n) => n as usize,
                Err(_) => 0,
            },
            _ => 0,
        }
    }

    fn variable_kind(x: SEXP) -> VariableKind {
        if x == unsafe { R_NilValue } {
            return VariableKind::Empty;
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
        let node = unsafe { Self::resolve_object_from_path(env, &path)? };

        match node {
            EnvironmentVariableNode::Artificial { object, name } => match name.as_str() {
                "<private>" => {
                    let env = Environment::new(object);
                    let enclos = Environment::new(RObject::view(env.find(".__enclos_env__")));
                    let private = RObject::view(enclos.find("private"));

                    Self::inspect_environment(private)
                },

                "<methods>" => Self::inspect_r6_methods(object),

                _ => Err(harp::error::Error::InspectError { path: path.clone() }),
            },

            EnvironmentVariableNode::Concrete { object } => {
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
            EnvironmentVariableNode::VectorElement { .. } => Ok(vec![]),
        }
    }

    pub fn clip(
        env: RObject,
        path: &Vec<String>,
        _format: &ClipboardFormatFormat,
    ) -> Result<String, harp::error::Error> {
        let node = unsafe { Self::resolve_object_from_path(env, &path)? };

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
            EnvironmentVariableNode::Artificial { .. } => Ok(String::from("")),
            EnvironmentVariableNode::VectorElement { object, index } => {
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
        let resolved = unsafe { Self::resolve_object_from_path(env, path)? };

        match resolved {
            EnvironmentVariableNode::Concrete { object } => Ok(object),

            _ => Err(harp::error::Error::InspectError { path: path.clone() }),
        }
    }

    unsafe fn resolve_object_from_path(
        object: RObject,
        path: &Vec<String>,
    ) -> Result<EnvironmentVariableNode, harp::error::Error> {
        let mut node = EnvironmentVariableNode::Concrete { object };

        for path_element in path {
            node = match node {
                EnvironmentVariableNode::Concrete { object } => {
                    if object.is_s4() {
                        let name = r_symbol!(path_element);
                        let child = r_try_catch(|| R_do_slot(*object, name))?;
                        EnvironmentVariableNode::Concrete { object: child }
                    } else {
                        let rtype = r_typeof(*object);
                        match rtype {
                            ENVSXP => {
                                if r_inherits(*object, "R6") && path_element.starts_with("<") {
                                    EnvironmentVariableNode::Artificial {
                                        object,
                                        name: path_element.clone(),
                                    }
                                } else {
                                    let symbol = r_symbol!(path_element);
                                    let mut x = Rf_findVarInFrame(*object, symbol);

                                    if r_typeof(x) == PROMSXP {
                                        // if we are here, it means the promise is either evaluated
                                        // already, i.e. PRVALUE() is bound or it is a promise to
                                        // something that is not a call or a symbol because it would
                                        // have been handled in Binding::new()

                                        // Actual promises, i.e. unevaluated promises can't be
                                        // expanded in the variables pane so we would not get here.

                                        let value = PRVALUE(x);
                                        if r_is_unbound(value) {
                                            x = PRCODE(x);
                                        } else {
                                            x = value;
                                        }
                                    }

                                    EnvironmentVariableNode::Concrete {
                                        object: RObject::view(x),
                                    }
                                }
                            },

                            VECSXP | EXPRSXP => {
                                let index = path_element.parse::<isize>().unwrap();
                                EnvironmentVariableNode::Concrete {
                                    object: RObject::view(VECTOR_ELT(*object, index)),
                                }
                            },

                            LISTSXP => {
                                let mut pairlist = *object;
                                let index = path_element.parse::<isize>().unwrap();
                                for _i in 0..index {
                                    pairlist = CDR(pairlist);
                                }
                                EnvironmentVariableNode::Concrete {
                                    object: RObject::view(CAR(pairlist)),
                                }
                            },

                            LGLSXP | RAWSXP | STRSXP | INTSXP | REALSXP | CPLXSXP => {
                                if r_is_matrix(*object) {
                                    EnvironmentVariableNode::Matrixcolumn {
                                        object,
                                        index: path_element.parse::<isize>().unwrap(),
                                    }
                                } else {
                                    EnvironmentVariableNode::VectorElement {
                                        object,
                                        index: path_element.parse::<isize>().unwrap(),
                                    }
                                }
                            },

                            _ => {
                                return Err(harp::error::Error::InspectError { path: path.clone() })
                            },
                        }
                    }
                },

                EnvironmentVariableNode::Artificial { object, name } => {
                    match name.as_str() {
                        "<private>" => {
                            let env = Environment::new(object);
                            let enclos =
                                Environment::new(RObject::view(env.find(".__enclos_env__")));
                            let private = Environment::new(RObject::view(enclos.find("private")));

                            // TODO: it seems unlikely that private would host active bindings
                            //       so find() is fine, we can assume this is concrete
                            EnvironmentVariableNode::Concrete {
                                object: RObject::view(private.find(path_element)),
                            }
                        },

                        _ => return Err(harp::error::Error::InspectError { path: path.clone() }),
                    }
                },

                EnvironmentVariableNode::VectorElement { .. } => {
                    return Err(harp::error::Error::InspectError { path: path.clone() });
                },

                EnvironmentVariableNode::Matrixcolumn { object, index } => unsafe {
                    let dim = IntegerVector::new(Rf_getAttrib(*object, R_DimSymbol))?;
                    let n_row = dim.get_unchecked(0).unwrap() as isize;

                    // TODO: use ? here, but this does not return a crate::error::Error, so
                    //       maybe use anyhow here instead ?
                    let row_index = path_element.parse::<isize>().unwrap();

                    EnvironmentVariableNode::VectorElement {
                        object,
                        index: n_row * index + row_index,
                    }
                },
            }
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
                });
            }

            Ok(out)
        }
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
            });
        }

        Ok(childs)
    }

    fn inspect_environment(value: RObject) -> Result<Vec<Variable>, harp::error::Error> {
        let mut out: Vec<Variable> = Environment::new(value)
            .iter()
            .filter(|b: &Binding| !b.is_hidden())
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
                let slot = r_try_catch(|| R_do_slot(value, slot_symbol))?;
                let access_key = display_name.clone();
                out.push(PositronVariable::from(access_key, display_name, *slot).var());
            }
        }

        Ok(out)
    }

    fn inspect_r6_methods(value: RObject) -> Result<Vec<Variable>, harp::error::Error> {
        let mut out: Vec<Variable> = Environment::new(value)
            .iter()
            .filter(|b: &Binding| match &b.value {
                BindingValue::Standard { object, .. } => r_typeof(object.sexp) == CLOSXP,

                _ => false,
            })
            .map(|b| Self::new(&b).var())
            .collect();

        out.sort_by(|a, b| a.display_name.cmp(&b.display_name));

        Ok(out)
    }
}
