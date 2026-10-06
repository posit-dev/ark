//
// r_object_explorer.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::collections::HashSet;
use std::ops::Range;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::object_explorer_comm::ChildrenResult;
use amalthea::comm::object_explorer_comm::FormattedValue;
use amalthea::comm::object_explorer_comm::ObjectExplorerBackendReply;
use amalthea::comm::object_explorer_comm::ObjectExplorerBackendRequest;
use amalthea::comm::object_explorer_comm::ObjectExplorerFrontendEvent;
use amalthea::comm::object_explorer_comm::ObjectExplorerState;
use amalthea::comm::object_explorer_comm::ObjectNode;
use amalthea::comm::object_explorer_comm::ObjectNodeKind;
use amalthea::comm::object_explorer_comm::SearchParams;
use amalthea::comm::object_explorer_comm::SearchResult;
use amalthea::comm::object_explorer_comm::SearchRow;
use amalthea::comm::object_explorer_comm::SearchRowMatchKind;
use amalthea::comm::variables_comm::ClipboardFormatFormat;
use amalthea::comm::variables_comm::Variable;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::r_symbol;
use harp::utils::r_chr_get_owned_utf8;
use harp::utils::r_inherits;
use harp::utils::r_is_data_frame;
use harp::utils::r_is_matrix;
use harp::utils::r_is_promise;
use harp::utils::r_is_s4;
use harp::utils::r_promise_is_forced;
use harp::utils::r_promise_value;
use harp::utils::r_typeof;
use libr::*;
use stdext::unwrap;

use crate::comm_handler::handle_rpc_request;
use crate::comm_handler::CommHandler;
use crate::comm_handler::CommHandlerContext;
use crate::comm_handler::EnvironmentChanged;
use crate::console::Console;
use crate::data_explorer::r_data_explorer::DataExplorerMode;
use crate::data_explorer::r_data_explorer::DataObjectEnvInfo;
use crate::data_explorer::r_data_explorer::RDataExplorer;
use crate::data_explorer::r_data_explorer::DATA_EXPLORER_COMM_NAME;
use crate::variables::variable::is_explorable;
use crate::variables::variable::parse_custom_access_key;
use crate::variables::variable::parse_index;
use crate::variables::variable::EnvironmentVariableNode;
use crate::variables::variable::PositronVariable;

pub const OBJECT_EXPLORER_COMM_NAME: &str = "positron.objectExplorer";
pub const POSITRON_OBJECT_EXPLORER_MIME: &str = "application/vnd.positron.objectExplorer+json";

/// The most nodes a search visits before it stops early.
const SEARCH_NODE_BUDGET: usize = 200_000;

/// The R backend for Positron's Object Explorer: serves one R object, one
/// level and one page of children at a time.
pub struct RObjectExplorer {
    /// The title of the explorer.
    title: String,

    /// The explored object.
    root: RObject,

    /// R code that evaluates to the explored object, if any.
    root_accessor: Option<String>,

    /// The top-level variable the explored object was read from, if any. The
    /// explorer is updated when the variable is reassigned and closed when it
    /// is removed.
    binding: Option<DataObjectEnvInfo>,

    /// The access key path from the binding's value to the explored object.
    path_in_binding: Vec<String>,

    /// Whether the explorer is shown inline only, rather than in an editor.
    inline: bool,
}

impl std::fmt::Debug for RObjectExplorer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RObjectExplorer")
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

impl RObjectExplorer {
    /// Creates an object explorer. Must be called from the R thread.
    ///
    /// - `title`: The title of the explorer.
    /// - `root`: The object to explore.
    /// - `root_accessor`: R code that evaluates to the object, if any.
    /// - `binding`: The variable the object was read from, if any, and the
    ///   access key path from the variable's value to the object.
    /// - `inline`: Whether the explorer is shown inline only.
    pub fn new(
        title: String,
        root: RObject,
        root_accessor: Option<String>,
        binding: Option<(DataObjectEnvInfo, Vec<String>)>,
        inline: bool,
    ) -> Self {
        let (binding, path_in_binding) = match binding {
            Some((binding, path)) => (Some(binding), path),
            None => (None, vec![]),
        };
        Self {
            title,
            root,
            root_accessor,
            binding,
            path_in_binding,
            inline,
        }
    }

    /// Reads the current value of a binding without forcing promises.
    /// Returns `None` if the binding is gone or holds an unforced promise.
    fn binding_value(binding: &DataObjectEnvInfo) -> Option<RObject> {
        let value = unsafe { Rf_findVarInFrame(binding.env.sexp, r_symbol!(binding.name)) };
        if value == unsafe { R_UnboundValue } {
            return None;
        }
        if r_is_promise(value) {
            if !r_promise_is_forced(value) {
                return None;
            }
            return Some(RObject::new(r_promise_value(value)));
        }
        Some(RObject::new(value))
    }

    /// Checks the binding for a new value. Returns false if the explorer
    /// should close because the explored object is gone.
    fn update(&mut self, ctx: &CommHandlerContext) -> anyhow::Result<bool> {
        let Some(binding) = &self.binding else {
            return Ok(true);
        };

        let current = unsafe { Rf_findVarInFrame(binding.env.sexp, r_symbol!(binding.name)) };
        if current == unsafe { R_UnboundValue } {
            return Ok(false);
        }

        // Never force a promise from a comm update; wait for it to be forced.
        if r_is_promise(current) && !r_promise_is_forced(current) {
            return Ok(true);
        }

        let Some(value) = Self::binding_value(binding) else {
            return Ok(true);
        };
        let node = PositronVariable::resolve_object_from_path(value, &self.path_in_binding);
        let Ok(EnvironmentVariableNode::Concrete { object }) = node else {
            return Ok(false);
        };
        if !is_explorable(object.sexp) {
            return Ok(false);
        }

        // An environment (including an R6 object) changes in place, so it is
        // always updated.
        if object.sexp == self.root.sexp && r_typeof(object.sexp) != ENVSXP {
            return Ok(true);
        }

        self.root = object;
        ctx.send_event(&ObjectExplorerFrontendEvent::Update);
        Ok(true)
    }

    fn handle_rpc(
        &mut self,
        request: ObjectExplorerBackendRequest,
    ) -> anyhow::Result<ObjectExplorerBackendReply> {
        match request {
            ObjectExplorerBackendRequest::GetState => Ok(
                ObjectExplorerBackendReply::GetStateReply(ObjectExplorerState {
                    title: self.title.clone(),
                    connected: true,
                    error_message: None,
                }),
            ),

            ObjectExplorerBackendRequest::GetRoot => {
                let var =
                    PositronVariable::from(String::new(), self.title.clone(), self.root.sexp).var();
                Ok(ObjectExplorerBackendReply::GetRootReply(object_node(
                    var,
                    self.root_accessor.clone(),
                    false,
                )))
            },

            ObjectExplorerBackendRequest::GetChildren(params) => {
                let start = params.start.max(0) as usize;
                let end = start.saturating_add(params.limit.max(0) as usize);
                let (children, total) = self.children(&params.path, start..end)?;
                Ok(ObjectExplorerBackendReply::GetChildrenReply(
                    ChildrenResult {
                        children,
                        total: total as i64,
                    },
                ))
            },

            ObjectExplorerBackendRequest::Search(params) => {
                Ok(ObjectExplorerBackendReply::SearchReply(self.search(params)))
            },

            ObjectExplorerBackendRequest::FormatValue(params) => {
                Ok(ObjectExplorerBackendReply::FormatValueReply(
                    self.format_value(&params.path, params.max_length)?,
                ))
            },

            ObjectExplorerBackendRequest::ViewTable(params) => {
                Ok(ObjectExplorerBackendReply::ViewTableReply(
                    self.view_table(&params.path, params.title)?,
                ))
            },

            // Opens a full explorer on the same object, e.g. from an inline one.
            ObjectExplorerBackendRequest::OpenObjectExplorer => {
                let explorer = RObjectExplorer::new(
                    self.title.clone(),
                    self.root.clone(),
                    self.root_accessor.clone(),
                    None,
                    false,
                );
                let id = Console::get_mut()
                    .comm_open_backend(OBJECT_EXPLORER_COMM_NAME, Box::new(explorer))?;
                Ok(ObjectExplorerBackendReply::OpenObjectExplorerReply(id))
            },
        }
    }

    /// Resolves the node at a path, along with its accessor and the
    /// environments from the root to it (the only objects that can contain
    /// themselves).
    fn resolve(
        &self,
        path: &[String],
    ) -> anyhow::Result<(EnvironmentVariableNode, Option<String>, HashSet<SEXP>)> {
        let mut node = EnvironmentVariableNode::Concrete {
            object: self.root.clone(),
        };
        let mut accessor = self.root_accessor.clone();
        let mut environments = HashSet::new();
        insert_environment(&node, &mut environments);

        for key in path {
            let selector = ChildSelector::new(&node).selector(key);
            accessor = accessor.zip(selector).map(|(a, s)| a + &s);
            node = PositronVariable::get_child_node_at(node, key)?;
            insert_environment(&node, &mut environments);
        }

        Ok((node, accessor, environments))
    }

    /// Gets the nodes of the children of the object at `path` in `range`, and
    /// the total number of children.
    fn children(
        &self,
        path: &[String],
        range: Range<usize>,
    ) -> anyhow::Result<(Vec<ObjectNode>, usize)> {
        let (node, accessor, environments) = self.resolve(path)?;
        let selector = ChildSelector::new(&node);
        let (variables, total) =
            PositronVariable::inspect_children(self.root.clone(), path, range)?;

        let nodes = variables
            .into_iter()
            .map(|var| {
                let child_accessor = accessor
                    .as_ref()
                    .zip(selector.selector(&var.access_key))
                    .map(|(a, s)| format!("{a}{s}"));
                let is_cycle = self.is_cycle(path, &var, &environments);
                object_node(var, child_accessor, is_cycle)
            })
            .collect();

        Ok((nodes, total))
    }

    /// Whether a child is an environment that is also one of its ancestors.
    fn is_cycle(
        &self,
        parent_path: &[String],
        child: &Variable,
        environments: &HashSet<SEXP>,
    ) -> bool {
        if !child.has_children {
            return false;
        }
        let mut path = parent_path.to_vec();
        path.push(child.access_key.clone());
        match PositronVariable::resolve_object_from_path(self.root.clone(), &path) {
            Ok(EnvironmentVariableNode::Concrete { object }) => {
                r_typeof(object.sexp) == ENVSXP && environments.contains(&object.sexp)
            },
            _ => false,
        }
    }

    /// Searches names and leaf values, depth first, returning matches and the
    /// ancestors that lead to them, in pre-order.
    fn search(&self, params: SearchParams) -> SearchResult {
        let mut search = Search {
            needle: params.query.to_lowercase(),
            max_depth: params.max_depth.max(0) as usize,
            max_results: params.max_results.max(0) as usize,
            rows: vec![],
            pending: vec![],
            matches: 0,
            visited: 0,
            truncated: false,
        };
        self.search_children(&mut search, &[]);
        SearchResult {
            rows: search.rows,
            total_matches: search.matches as i64,
            truncated: search.truncated,
        }
    }

    /// Visits the children of the object at `path` and their descendants.
    /// Returns true when the search must stop.
    fn search_children(&self, search: &mut Search, path: &[String]) -> bool {
        let remaining = SEARCH_NODE_BUDGET.saturating_sub(search.visited);
        let children = match self.children(path, 0..remaining + 1) {
            Ok((children, _)) => children,
            Err(err) => {
                log::warn!("Object explorer search skipped {path:?}: {err}");
                return false;
            },
        };

        for node in children {
            if search.visited >= SEARCH_NODE_BUDGET || search.matches >= search.max_results {
                search.truncated = true;
                return true;
            }
            search.visited += 1;

            let mut child_path = path.to_vec();
            child_path.push(node.access_key.clone());

            // A truncated display value is matched on the full value instead.
            let is_leaf = !node.has_children && !node.is_cycle;
            let full_value = if is_leaf && node.is_truncated {
                self.format_text(&child_path).ok()
            } else {
                None
            };
            let match_kind = search.match_kind(&node, full_value.as_deref());
            let matched = match_kind.is_some();
            if let Some(match_kind) = match_kind {
                for (ancestor_path, ancestor_node, emitted) in search.pending.iter_mut() {
                    if !*emitted {
                        search.rows.push(SearchRow {
                            path: ancestor_path.clone(),
                            node: ancestor_node.clone(),
                            match_kind: SearchRowMatchKind::Ancestor,
                        });
                        *emitted = true;
                    }
                }
                search.rows.push(SearchRow {
                    path: child_path.clone(),
                    node: node.clone(),
                    match_kind,
                });
                search.matches += 1;
            }

            if child_path.len() < search.max_depth && node.has_children {
                search.pending.push((child_path.clone(), node, matched));
                let stop = self.search_children(search, &child_path);
                search.pending.pop();
                if stop {
                    return true;
                }
            }
        }

        false
    }

    /// Opens a data explorer on the table at `path`, returning its comm id.
    fn view_table(&self, path: &[String], title: String) -> anyhow::Result<String> {
        let object = match PositronVariable::resolve_object_from_path(self.root.clone(), path)? {
            EnvironmentVariableNode::Concrete { object }
                if harp::table_kind(object.sexp).is_some() =>
            {
                object
            },
            _ => anyhow::bail!("Can't view {path:?} as a table"),
        };
        let explorer = RDataExplorer::new(title, object, None, DataExplorerMode::Full)?;
        Console::get_mut().comm_open_backend(DATA_EXPLORER_COMM_NAME, Box::new(explorer))
    }

    /// Formats the value of the object at `path` as plain text, cut at
    /// `max_length` characters.
    fn format_value(
        &self,
        path: &[String],
        max_length: Option<i64>,
    ) -> anyhow::Result<FormattedValue> {
        let content = self.format_text(path)?;
        let cut = max_length.and_then(|n| content.char_indices().nth(n.max(0) as usize));
        Ok(match cut {
            Some((index, _)) => FormattedValue {
                content: content[..index].to_string(),
                is_truncated: true,
            },
            None => FormattedValue {
                content,
                is_truncated: false,
            },
        })
    }

    /// Formats the full value of the object at `path` as plain text. A string
    /// is its contents, without quotes or escapes.
    fn format_text(&self, path: &[String]) -> anyhow::Result<String> {
        let node = PositronVariable::resolve_object_from_path(self.root.clone(), path)?;
        let string = match &node {
            EnvironmentVariableNode::Concrete { object }
                if r_typeof(object.sexp) == STRSXP && object.length() == 1 =>
            {
                Some((object, 0))
            },
            EnvironmentVariableNode::AtomicVectorElement { object, index }
                if r_typeof(object.sexp) == STRSXP =>
            {
                Some((object, *index))
            },
            _ => None,
        };
        // A missing string is formatted as NA below.
        if let Some(Ok(string)) =
            string.map(|(object, index)| r_chr_get_owned_utf8(object.sexp, index))
        {
            return Ok(string);
        }
        if let EnvironmentVariableNode::Concrete { object } = &node {
            let printable = match r_typeof(object.sexp) {
                LGLSXP | INTSXP | REALSXP | CPLXSXP | STRSXP | RAWSXP | CLOSXP => false,
                _ => !r_is_data_frame(object.sexp),
            };
            if printable || r_is_s4(object.sexp) {
                return Ok(RFunction::from(".ps.object_explorer.format_value")
                    .add(object.clone())
                    .call()?
                    .try_into()?);
            }
        }
        PositronVariable::clip(self.root.clone(), path, &ClipboardFormatFormat::TextPlain)
    }

    /// The variable path of the explored object, for the frontend to match
    /// Variables pane items to open explorers.
    fn variable_path(&self) -> Option<Vec<String>> {
        let binding = self.binding.as_ref()?;
        let mut path = vec![binding.name.clone()];
        path.extend(self.path_in_binding.iter().cloned());
        Some(path)
    }
}

impl CommHandler for RObjectExplorer {
    fn open_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "title": self.title,
            "inline_only": self.inline,
            "variable_path": self.variable_path(),
        })
    }

    fn handle_msg(&mut self, msg: CommMsg, ctx: &CommHandlerContext) {
        handle_rpc_request(&ctx.outgoing_tx, OBJECT_EXPLORER_COMM_NAME, msg, |req| {
            self.handle_rpc(req)
        });
    }

    fn handle_environment(&mut self, event: &EnvironmentChanged, ctx: &CommHandlerContext) {
        let EnvironmentChanged::Execution { .. } = event else {
            return;
        };
        match self.update(ctx) {
            Ok(true) => {},
            Ok(false) => ctx.close_on_exit(),
            Err(err) => log::error!("Error while checking for object explorer update: {err}"),
        }
    }
}

/// The state of a search in progress.
struct Search {
    needle: String,
    max_depth: usize,
    max_results: usize,
    rows: Vec<SearchRow>,
    /// The ancestors of the node being visited, below the root, and whether
    /// each has been emitted.
    pending: Vec<(Vec<String>, ObjectNode, bool)>,
    matches: usize,
    visited: usize,
    truncated: bool,
}

impl Search {
    /// How a node matches the search, if it does. Values match only on
    /// leaves, whose display value is the value rather than a summary.
    /// `full_value` replaces the display value when it is given.
    fn match_kind(
        &self,
        node: &ObjectNode,
        full_value: Option<&str>,
    ) -> Option<SearchRowMatchKind> {
        let name_match = node.display_name.to_lowercase().contains(&self.needle);
        let is_leaf = !node.has_children && !node.is_cycle;
        let value = full_value.unwrap_or(&node.display_value);
        let value_match = is_leaf && value.to_lowercase().contains(&self.needle);
        match (name_match, value_match) {
            (true, true) => Some(SearchRowMatchKind::NameAndValue),
            (true, false) => Some(SearchRowMatchKind::Name),
            (false, true) => Some(SearchRowMatchKind::Value),
            (false, false) => None,
        }
    }
}

/// Builds R code that selects children of an object, following the
/// conventions R users write by hand: `[["name"]]` for uniquely named list
/// elements and bindings, `[[i]]` otherwise, `$name` for R6 fields, `@name`
/// for S4 slots, and `[i]` / `[, j]` for vector elements and matrix columns.
/// `$` is never used for lists because it matches partially.
enum ChildSelector {
    /// A list or atomic vector, whose elements are selected by name when the
    /// name is unique and non-empty, and otherwise by position with `[[i]]`
    /// (lists) or `[i]` (atomic vectors).
    Named {
        names: Vec<Option<String>>,
        counts: HashMap<String, usize>,
        atomic: bool,
    },
    /// A pairlist, whose elements are selected by position.
    Pairlist,
    /// A matrix, whose children are its columns.
    Matrix,
    /// A matrix column, whose children are its elements.
    MatrixColumn,
    /// An environment, whose children are its bindings.
    Environment,
    /// An R6 object, whose public fields are selected with `$`.
    R6,
    /// An S4 object, whose slots are selected with `@`.
    S4,
    /// Children that no R code selects, such as R6 private fields.
    None,
}

impl ChildSelector {
    fn new(node: &EnvironmentVariableNode) -> Self {
        let object = match node {
            EnvironmentVariableNode::Concrete { object } => object,
            EnvironmentVariableNode::Matrixcolumn { .. } => return Self::MatrixColumn,
            _ => return Self::None,
        };

        if r_is_s4(object.sexp) {
            return Self::S4;
        }
        match r_typeof(object.sexp) {
            ENVSXP if r_inherits(object.sexp, "R6") => Self::R6,
            ENVSXP => Self::Environment,
            LISTSXP => Self::Pairlist,
            LGLSXP | INTSXP | REALSXP | CPLXSXP | STRSXP | RAWSXP if r_is_matrix(object.sexp) => {
                Self::Matrix
            },
            VECSXP | EXPRSXP | LGLSXP | INTSXP | REALSXP | CPLXSXP | STRSXP | RAWSXP => {
                let names = object.names().unwrap_or_default();
                let mut counts = HashMap::new();
                for name in names.iter().flatten() {
                    *counts.entry(name.clone()).or_insert(0) += 1;
                }
                Self::Named {
                    names,
                    counts,
                    atomic: !matches!(r_typeof(object.sexp), VECSXP | EXPRSXP),
                }
            },
            _ => Self::None,
        }
    }

    /// The R code selecting the child with an access key, if any.
    fn selector(&self, access_key: &str) -> Option<String> {
        if matches!(parse_custom_access_key(access_key), Ok(Some(_))) {
            return None;
        }
        let position = || parse_index(access_key).ok().map(|i| i + 1);
        match self {
            Self::Named {
                names,
                counts,
                atomic,
            } => {
                let index = parse_index(access_key).ok()? as usize;
                match names.get(index).cloned().flatten() {
                    Some(name) if !name.is_empty() && counts.get(&name) == Some(&1) => {
                        Some(format!("[[{}]]", r_string_literal(&name)))
                    },
                    _ if *atomic => Some(format!("[{}]", index + 1)),
                    _ => Some(format!("[[{}]]", index + 1)),
                }
            },
            Self::Pairlist => Some(format!("[[{}]]", position()?)),
            Self::Matrix => Some(format!("[, {}]", position()?)),
            Self::MatrixColumn => Some(format!("[{}]", position()?)),
            Self::Environment => Some(format!("[[{}]]", r_string_literal(access_key))),
            Self::R6 if access_key.starts_with('<') => None,
            Self::R6 => Some(format!("${}", r_name(access_key))),
            Self::S4 => Some(format!("@{}", r_name(access_key))),
            Self::None => None,
        }
    }
}

/// Records the node's object if it is an environment.
fn insert_environment(node: &EnvironmentVariableNode, environments: &mut HashSet<SEXP>) {
    if let EnvironmentVariableNode::Concrete { object } = node {
        if r_typeof(object.sexp) == ENVSXP {
            environments.insert(object.sexp);
        }
    }
}

/// Builds an object explorer node from a Variables pane variable.
fn object_node(var: Variable, accessor: Option<String>, is_cycle: bool) -> ObjectNode {
    // Both enums serialize to the same strings.
    let kind = serde_json::to_value(&var.kind)
        .and_then(serde_json::from_value)
        .unwrap_or(ObjectNodeKind::Other);
    ObjectNode {
        access_key: var.access_key,
        display_name: var.display_name,
        display_type: var.display_type,
        display_value: var.display_value,
        kind,
        length: var.length,
        has_children: var.has_children && !is_cycle,
        is_truncated: var.is_truncated,
        is_cycle,
        accessor,
    }
}

/// Formats a string as an R string literal.
fn r_string_literal(value: &str) -> String {
    let mut literal = String::with_capacity(value.len() + 2);
    literal.push('"');
    for c in value.chars() {
        match c {
            '"' => literal.push_str("\\\""),
            '\\' => literal.push_str("\\\\"),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            '\t' => literal.push_str("\\t"),
            c => literal.push(c),
        }
    }
    literal.push('"');
    literal
}

/// Formats a name for use after `$` or `@`, backquoting it if it isn't
/// syntactic.
fn r_name(name: &str) -> String {
    if is_syntactic(name) {
        name.to_string()
    } else {
        format!("`{}`", name.replace('\\', "\\\\").replace('`', "\\`"))
    }
}

/// Whether a name is syntactic in R, i.e. usable without backquotes.
fn is_syntactic(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "if",
        "else",
        "repeat",
        "while",
        "function",
        "for",
        "next",
        "break",
        "TRUE",
        "FALSE",
        "NULL",
        "Inf",
        "NaN",
        "NA",
        "NA_integer_",
        "NA_real_",
        "NA_character_",
        "NA_complex_",
        "in",
    ];
    let mut chars = name.chars();
    let valid_start = match chars.next() {
        Some('.') => !name.chars().nth(1).is_some_and(|c| c.is_ascii_digit()),
        Some(c) => c.is_alphabetic(),
        None => false,
    };
    valid_start &&
        chars.all(|c| c.is_alphanumeric() || c == '.' || c == '_') &&
        !RESERVED.contains(&name)
}

/// Opens an R object in the Object Explorer. Called from `View()`.
///
/// - `x`: The object.
/// - `title`: The title of the explorer.
/// - `var`: The name of the variable holding the object in `env`, or `""`.
/// - `env`: The environment holding the variable, or `NULL`.
/// - `accessor`: R code that evaluates to the object, or `""`.
#[harp::register]
pub unsafe extern "C-unwind" fn ps_view_object(
    x: SEXP,
    title: SEXP,
    var: SEXP,
    env: SEXP,
    accessor: SEXP,
) -> anyhow::Result<SEXP> {
    let title = unwrap!(String::try_from(RObject::view(title)), Err(_) => String::new());
    let var = unwrap!(String::try_from(RObject::view(var)), Err(_) => String::new());
    let accessor = unwrap!(String::try_from(RObject::view(accessor)), Err(_) => String::new());

    let binding = if env != R_NilValue && !var.is_empty() {
        Some((
            DataObjectEnvInfo {
                name: var,
                env: RObject::new(env),
            },
            vec![],
        ))
    } else {
        None
    };
    let accessor = if accessor.is_empty() {
        title.clone()
    } else {
        accessor
    };

    let explorer = RObjectExplorer::new(title, RObject::new(x), Some(accessor), binding, false);
    Console::get_mut().comm_open_backend(OBJECT_EXPLORER_COMM_NAME, Box::new(explorer))?;

    Ok(R_NilValue)
}

/// Whether a printed value is nested data worth exploring inline in a
/// notebook: a non-empty list (but not a data frame or matrix), or an
/// environment other than the global one.
pub(crate) fn is_inline_explorable(value: SEXP) -> bool {
    if r_is_data_frame(value) || r_is_matrix(value) {
        return false;
    }
    match r_typeof(value) {
        VECSXP => unsafe { Rf_xlength(value) > 0 },
        ENVSXP => value != unsafe { R_GlobalEnv },
        _ => false,
    }
}

/// Builds the R code that selects the object at an access key path below a
/// named object, if there is such code.
pub(crate) fn path_accessor(name: &str, object: RObject, path: &[String]) -> Option<String> {
    let mut node = EnvironmentVariableNode::Concrete { object };
    let mut accessor = name.to_string();
    for key in path {
        accessor += &ChildSelector::new(&node).selector(key)?;
        node = PositronVariable::get_child_node_at(node, key).ok()?;
    }
    Some(accessor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::package_is_installed;
    use crate::r_task;

    fn explorer(code: &str) -> RObjectExplorer {
        let value = harp::parse_eval_global(code).unwrap();
        RObjectExplorer::new(
            String::from("x"),
            value,
            Some(String::from("x")),
            None,
            false,
        )
    }

    fn to_path(path: &[&str]) -> Vec<String> {
        path.iter().map(|key| key.to_string()).collect()
    }

    fn children(explorer: &RObjectExplorer, path: &[&str]) -> Vec<ObjectNode> {
        explorer.children(&to_path(path), 0..1000).unwrap().0
    }

    fn names(nodes: &[ObjectNode]) -> Vec<String> {
        nodes.iter().map(|node| node.display_name.clone()).collect()
    }

    fn accessors(nodes: &[ObjectNode]) -> Vec<Option<String>> {
        nodes.iter().map(|node| node.accessor.clone()).collect()
    }

    fn some(values: &[&str]) -> Vec<Option<String>> {
        values.iter().map(|value| Some(value.to_string())).collect()
    }

    #[test]
    fn test_object_explorer_list_accessors() {
        r_task(|| {
            let explorer =
                explorer(r#"list(a = 1, b = list(c = 2), 3, d = 4, d = 5, `odd "name"` = 6)"#);

            let nodes = children(&explorer, &[]);
            assert_eq!(
                accessors(&nodes),
                some(&[
                    r#"x[["a"]]"#,
                    r#"x[["b"]]"#,
                    "x[[3]]",
                    "x[[4]]",
                    "x[[5]]",
                    r#"x[["odd \"name\""]]"#,
                ])
            );
            assert_eq!(
                accessors(&children(&explorer, &["1"])),
                some(&[r#"x[["b"]][["c"]]"#])
            );
        })
    }

    #[test]
    fn test_object_explorer_vector_and_matrix_accessors() {
        r_task(|| {
            let explorer = explorer("list(v = c(a = 1, b = 2), u = 1:2, m = matrix(1:4, 2))");

            assert_eq!(
                accessors(&children(&explorer, &["0"])),
                some(&[r#"x[["v"]][["a"]]"#, r#"x[["v"]][["b"]]"#])
            );
            assert_eq!(
                accessors(&children(&explorer, &["1"])),
                some(&[r#"x[["u"]][1]"#, r#"x[["u"]][2]"#])
            );
            assert_eq!(
                accessors(&children(&explorer, &["2"])),
                some(&[r#"x[["m"]][, 1]"#, r#"x[["m"]][, 2]"#])
            );
            assert_eq!(
                accessors(&children(&explorer, &["2", "1"])),
                some(&[r#"x[["m"]][, 2][1]"#, r#"x[["m"]][, 2][2]"#])
            );
        })
    }

    #[test]
    fn test_object_explorer_environment_and_pairlist() {
        r_task(|| {
            let explorer =
                explorer("local({ e <- new.env(); e$b <- 1; e$a <- pairlist(p = 1, 2); e })");

            let nodes = children(&explorer, &[]);
            assert_eq!(names(&nodes), vec!["a", "b"]);
            assert_eq!(accessors(&nodes), some(&[r#"x[["a"]]"#, r#"x[["b"]]"#]));
            assert_eq!(
                accessors(&children(&explorer, &["a"])),
                some(&[r#"x[["a"]][[1]]"#, r#"x[["a"]][[2]]"#])
            );
        })
    }

    #[test]
    fn test_object_explorer_environment_cycle() {
        r_task(|| {
            let explorer = explorer("local({ e <- new.env(); e$self <- e; e$n <- 1; e })");

            let nodes = children(&explorer, &[]);
            let cycle = nodes
                .iter()
                .find(|node| node.display_name == "self")
                .unwrap();
            assert!(cycle.is_cycle);
            assert!(!cycle.has_children);
        })
    }

    #[test]
    fn test_object_explorer_s4_slots() {
        r_task(|| {
            harp::parse_eval_global(
                r#"setClass("OEPoint", representation(x = "numeric", `bad name` = "numeric"))"#,
            )
            .unwrap();
            let explorer = explorer(r#"new("OEPoint", x = 1, `bad name` = 2)"#);

            assert_eq!(
                accessors(&children(&explorer, &[])),
                some(&["x@x", "x@`bad name`"])
            );
        })
    }

    #[test]
    fn test_object_explorer_r6() {
        r_task(|| {
            if !package_is_installed("R6") {
                return;
            }
            let explorer = explorer(
                "R6::R6Class('C', public = list(field = 1, method = function() 1), private = list(secret = 2))$new()",
            );

            let nodes = children(&explorer, &[]);
            assert_eq!(names(&nodes), vec!["field", "private", "methods"]);
            assert_eq!(accessors(&nodes), vec![
                Some(String::from("x$field")),
                None,
                None
            ]);
            assert_eq!(accessors(&children(&explorer, &["<private>"])), vec![None]);
        })
    }

    #[test]
    fn test_object_explorer_paging() {
        r_task(|| {
            let explorer = explorer("as.list(1:2500)");

            let (nodes, total) = explorer.children(&[], 1000..2000).unwrap();
            assert_eq!(total, 2500);
            assert_eq!(nodes.len(), 1000);
            assert_eq!(nodes[0].display_name, "[[1001]]");
            assert_eq!(nodes[999].access_key, "1999");
        })
    }

    #[test]
    fn test_object_explorer_format_value() {
        r_task(|| {
            let explorer =
                explorer(r#"list(v = 1:500, l = list(a = 1), s = "one\ntwo", c = c("x", NA))"#);
            let format = |key: &str, max_length: Option<i64>| {
                explorer.format_value(&to_path(&[key]), max_length).unwrap()
            };

            assert!(format("0", None).content.ends_with("500"));
            assert!(format("1", None).content.starts_with("$a"));
            assert_eq!(format("2", None).content, "one\ntwo");
            assert_eq!(
                (
                    format("2", Some(3)).content,
                    format("2", Some(3)).is_truncated
                ),
                (String::from("one"), true)
            );
            assert!(!format("2", Some(7)).is_truncated);

            let element = |index: &str| {
                explorer
                    .format_value(&to_path(&["3", index]), None)
                    .unwrap()
                    .content
            };
            assert_eq!(
                (element("0"), element("1")),
                (String::from("x"), String::from("NA"))
            );
        })
    }

    #[test]
    fn test_object_explorer_search() {
        r_task(|| {
            let explorer = explorer(
                r#"list(alpha = list(beta = list(1, "needle", list(gamma = "needle"))), delta = "haystack")"#,
            );
            let search = |max_depth: i64, max_results: i64| {
                explorer.search(SearchParams {
                    query: String::from("NEEDLE"),
                    max_depth,
                    max_results,
                })
            };

            let result = search(10, 1000);
            let rows: Vec<String> = result
                .rows
                .iter()
                .map(|row| format!("{}:{:?}", row.path.join("/"), row.match_kind))
                .collect();
            assert_eq!(rows, vec![
                "0:Ancestor",
                "0/0:Ancestor",
                "0/0/1:Value",
                "0/0/2:Ancestor",
                "0/0/2/0:Value",
            ]);
            assert_eq!(result.total_matches, 2);
            assert!(!result.truncated);

            assert_eq!(search(3, 1000).rows.len(), 3);
            assert!(search(10, 1).truncated);
        })
    }

    #[test]
    fn test_object_explorer_search_past_truncated_string() {
        r_task(|| {
            let explorer = explorer(r#"list(text = paste0(strrep("x", 2000), "needle"))"#);
            let result = explorer.search(SearchParams {
                query: String::from("needle"),
                max_depth: 10,
                max_results: 1000,
            });
            assert_eq!(result.total_matches, 1);
        })
    }

    #[test]
    fn test_object_explorer_is_explorable() {
        r_task(|| {
            let explorable =
                |code: &str| is_explorable(harp::parse_eval_global(code).unwrap().sexp);
            assert!(explorable("list(a = 1)"));
            assert!(explorable("new.env()"));
            assert!(explorable("pairlist(a = 1)"));
            assert!(explorable("expression(1 + 1)"));
            assert!(!explorable("mtcars"));
            assert!(!explorable("1:3"));
            assert!(!explorable("NULL"));
            assert!(!explorable("matrix(list(1, 2, 3, 4), 2)"));
        })
    }
}
