use std::fs;
use std::path::Path;

use quote::ToTokens;
use syn::punctuated::Punctuated;
use syn::Attribute;
use syn::Field;
use syn::Fields;
use syn::FnArg;
use syn::ImplItem;
use syn::Item;
use syn::ItemStruct;
use syn::Meta;
use syn::ReturnType;
use syn::Signature;
use syn::Stmt;
use syn::Token;
use walkdir::WalkDir;

pub const UPDATE_CHECKLIST: &str = "\
Updating this snapshot means the Salsa query surface changed. This is a review backstop for additions, signature changes, options, and recovery handlers. It does not detect dependency-edge changes. Before accepting:
- Determine whether the new or changed query can become Salsa's repeated key, not merely participate in a cycle. `semantic_index()`, `exports()`, `attached_packages()`, and `cross_file_layers()` re-enter each other while resolving `source()` and attach effects. `Package::resolve()` can recurse through NAMESPACE re-exports. `cross_file_layers()` re-enters at `attached_packages()`, so it has no handler of its own. Choose a fallback that is correct for the repeated query, which is often an empty value.
- Trace every production root through non-tracked helpers, `SalsaImportsResolver`, and `Db` / `DbInputs`. A query body alone misses recursive paths.
- These are known cycles, not an exhaustive list. Analyze any recursion the query introduces.
- State in the PR whether the query can participate in a cycle, why, and which production roots were examined.";
enum SalsaKind {
    Input,
    Interned,
    Tracked,
}

#[derive(Default)]
struct Inventory {
    inputs: Vec<StructEntry>,
    interned: Vec<StructEntry>,
    tracked_structs: Vec<StructEntry>,
    queries: Vec<QueryEntry>,
}

struct StructEntry {
    rel_path: String,
    header: String,
    fields: Vec<String>,
}

struct QueryEntry {
    rel_path: String,
    rendered_signature: String,
    qualified_name: String,
    cycle_recovery: Option<String>,
}

/// Attributes the structured walk finds on an item it does not otherwise inventory: the
/// attribute on a `#[salsa::tracked] impl` block, and salsa-attributed items excluded by
/// `#[cfg(test)]`. Both are still picked up by the text scan in `reconcile_attribute_count()`.
#[derive(Default)]
struct FileCounts {
    tracked_impl_blocks: usize,
    skipped_cfg_test: usize,
}

/// Returns a stable snapshot of Salsa query definitions in non-test Rust files below `source_dir`.
pub fn salsa_inventory(source_dir: &Path) -> String {
    let mut files = Vec::new();

    for entry in WalkDir::new(source_dir).sort_by_file_name() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => panic!("failed to walk {}: {err}", source_dir.display()),
        };
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }

        let rel_path = match path.strip_prefix(source_dir) {
            Ok(rel_path) => rel_path,
            Err(_) => panic!("{} is not under {}", path.display(), source_dir.display()),
        };
        let rel_path = match rel_path.to_str() {
            Some(rel_path) => rel_path.replace('\\', "/"),
            None => panic!("non-utf8 path: {}", path.display()),
        };

        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(err) => panic!("failed to read {}: {err}", path.display()),
        };

        files.push((rel_path, source));
    }

    render_from_sources(&files)
}

/// Parses and renders an inventory from in-memory `(relative_path, source_text)` pairs, applying
/// the same test-path exclusion `salsa_inventory()` applies when walking a directory.
fn render_from_sources(files: &[(String, String)]) -> String {
    let mut inventory = Inventory::default();

    for (rel_path, source) in files {
        if is_test_path(Path::new(rel_path)) {
            continue;
        }

        let file = match syn::parse_file(source) {
            Ok(file) => file,
            Err(err) => panic!("failed to parse {rel_path}: {err}"),
        };

        let queries_before = inventory.queries.len();
        let structs_before = struct_count(&inventory);

        let mut counts = FileCounts::default();
        collect_items(&file.items, rel_path, None, &mut inventory, &mut counts);

        let queries_added = inventory.queries.len() - queries_before;
        let structs_added = struct_count(&inventory) - structs_before;
        reconcile_attribute_count(source, rel_path, queries_added, structs_added, &counts);
    }

    render_report(inventory)
}

/// Test code defines its own tracked queries, which are not part of the production query surface.
fn is_test_path(rel_path: &Path) -> bool {
    if rel_path.file_stem().and_then(|stem| stem.to_str()) == Some("tests") {
        return true;
    }
    rel_path
        .components()
        .any(|component| component.as_os_str() == "tests")
}

fn struct_count(inventory: &Inventory) -> usize {
    inventory.inputs.len() + inventory.interned.len() + inventory.tracked_structs.len()
}

/// Cross-checks the structured walk against an independent line-anchored text scan, so a
/// declaration form the walk does not know about fails loudly instead of vanishing silently.
fn reconcile_attribute_count(
    source: &str,
    rel_path: &str,
    queries: usize,
    salsa_structs: usize,
    counts: &FileCounts,
) {
    let text_scanned = count_salsa_attribute_lines(source);
    let accounted = queries + counts.tracked_impl_blocks + salsa_structs + counts.skipped_cfg_test;
    if text_scanned != accounted {
        panic!(
            "{rel_path}: text scan found {text_scanned} salsa attribute(s) but the structured walk only accounts for {accounted} (queries={queries}, tracked_impl_blocks={}, salsa_structs={salsa_structs}, skipped_cfg_test={}); a salsa declaration form is present that the walk does not handle",
            counts.tracked_impl_blocks, counts.skipped_cfg_test,
        );
    }
}

const SALSA_ATTR_LINE_PREFIXES: [&str; 6] = [
    "#[salsa::tracked",
    "#[salsa::input",
    "#[salsa::interned",
    "#[salsa_macros::tracked",
    "#[salsa_macros::input",
    "#[salsa_macros::interned",
];

/// Line-anchored so a doc comment mentioning `#[salsa::tracked]` mid-line after `///` is not
/// mistaken for a declaration.
fn count_salsa_attribute_lines(source: &str) -> usize {
    source
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            SALSA_ATTR_LINE_PREFIXES
                .iter()
                .any(|prefix| trimmed.starts_with(prefix))
        })
        .count()
}

fn collect_items(
    items: &[Item],
    rel_path: &str,
    self_ty: Option<&str>,
    inventory: &mut Inventory,
    counts: &mut FileCounts,
) {
    for item in items {
        collect_item(item, rel_path, self_ty, inventory, counts);
    }
}

fn collect_item(
    item: &Item,
    rel_path: &str,
    self_ty: Option<&str>,
    inventory: &mut Inventory,
    counts: &mut FileCounts,
) {
    match item {
        Item::Fn(item_fn) => {
            if has_cfg_test(&item_fn.attrs) {
                counts.skipped_cfg_test += count_salsa_attributes_in_item(item);
                return;
            }
            check_item_attr_spelling(
                &item_fn.attrs,
                rel_path,
                &format!("fn {}", item_fn.sig.ident),
            );
            collect_tracked_fn(&item_fn.attrs, &item_fn.sig, self_ty, rel_path, inventory);
            collect_nested_items(&item_fn.block.stmts, rel_path, self_ty, inventory, counts);
        },
        Item::Impl(item_impl) => {
            if has_cfg_test(&item_impl.attrs) {
                counts.skipped_cfg_test += count_salsa_attributes_in_item(item);
                return;
            }
            let impl_self_ty = normalize_tokens(item_impl.self_ty.as_ref());
            check_item_attr_spelling(&item_impl.attrs, rel_path, &format!("impl {impl_self_ty}"));
            if item_impl
                .attrs
                .iter()
                .any(|attr| salsa_attr_kind(attr).is_some())
            {
                counts.tracked_impl_blocks += 1;
            }
            for impl_item in &item_impl.items {
                let ImplItem::Fn(impl_fn) = impl_item else {
                    continue;
                };
                if has_cfg_test(&impl_fn.attrs) {
                    counts.skipped_cfg_test +=
                        count_salsa_attributes_in_fn(&impl_fn.attrs, &impl_fn.block.stmts);
                    continue;
                }
                check_item_attr_spelling(
                    &impl_fn.attrs,
                    rel_path,
                    &format!("fn {impl_self_ty}::{}", impl_fn.sig.ident),
                );
                collect_tracked_fn(
                    &impl_fn.attrs,
                    &impl_fn.sig,
                    Some(&impl_self_ty),
                    rel_path,
                    inventory,
                );
                collect_nested_items(
                    &impl_fn.block.stmts,
                    rel_path,
                    Some(&impl_self_ty),
                    inventory,
                    counts,
                );
            }
        },
        Item::Struct(item_struct) => {
            if has_cfg_test(&item_struct.attrs) {
                counts.skipped_cfg_test += count_salsa_attributes_in_item(item);
                return;
            }
            check_item_attr_spelling(
                &item_struct.attrs,
                rel_path,
                &format!("struct {}", item_struct.ident),
            );
            collect_tracked_struct(item_struct, rel_path, inventory);
        },
        Item::Mod(item_mod) => {
            if has_cfg_test(&item_mod.attrs) {
                if let Some((_, items)) = &item_mod.content {
                    counts.skipped_cfg_test += count_salsa_attributes_in_items(items);
                }
                return;
            }
            if let Some((_, items)) = &item_mod.content {
                collect_items(items, rel_path, None, inventory, counts);
            }
        },
        _ => {},
    }
}

/// A `#[salsa::tracked] fn` can be declared inside another function's or method's body, where it
/// surfaces as `Stmt::Item` rather than a top-level `Item`.
fn collect_nested_items(
    stmts: &[Stmt],
    rel_path: &str,
    self_ty: Option<&str>,
    inventory: &mut Inventory,
    counts: &mut FileCounts,
) {
    for stmt in stmts {
        if let Stmt::Item(item) = stmt {
            collect_item(item, rel_path, self_ty, inventory, counts);
        }
    }
}

/// Counts salsa attributes under a `#[cfg(test)]`-gated item so that the reconciliation in
/// `reconcile_attribute_count()` still balances: the walk excludes this subtree from the
/// inventory, but the raw text scan does not respect `#[cfg(test)]` boundaries.
fn count_salsa_attributes_in_item(item: &Item) -> usize {
    match item {
        Item::Fn(item_fn) => count_salsa_attributes_in_fn(&item_fn.attrs, &item_fn.block.stmts),
        Item::Impl(item_impl) => {
            let mut count = is_salsa_attributed(&item_impl.attrs) as usize;
            for impl_item in &item_impl.items {
                if let ImplItem::Fn(impl_fn) = impl_item {
                    count += count_salsa_attributes_in_fn(&impl_fn.attrs, &impl_fn.block.stmts);
                }
            }
            count
        },
        Item::Struct(item_struct) => is_salsa_attributed(&item_struct.attrs) as usize,
        Item::Mod(item_mod) => match &item_mod.content {
            Some((_, items)) => count_salsa_attributes_in_items(items),
            None => 0,
        },
        _ => 0,
    }
}

fn count_salsa_attributes_in_fn(attrs: &[Attribute], stmts: &[Stmt]) -> usize {
    is_salsa_attributed(attrs) as usize + count_salsa_attributes_in_stmts(stmts)
}

fn count_salsa_attributes_in_stmts(stmts: &[Stmt]) -> usize {
    stmts
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Item(item) => Some(count_salsa_attributes_in_item(item)),
            _ => None,
        })
        .sum()
}

fn count_salsa_attributes_in_items(items: &[Item]) -> usize {
    items.iter().map(count_salsa_attributes_in_item).sum()
}

fn has_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg") &&
            matches!(&attr.meta, Meta::List(list) if normalize_tokens(&list.tokens) == "test")
    })
}

fn is_salsa_attributed(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| salsa_attr_kind(attr).is_some())
}

const BARE_SALSA_ATTR_NAMES: [&str; 3] = ["tracked", "input", "interned"];

/// Field attributes such as `#[tracked]` are legitimate (see `FIELD_SALSA_ATTRS`); this only
/// checks attributes on the item itself (fn, impl, struct).
fn check_item_attr_spelling(attrs: &[Attribute], rel_path: &str, item_desc: &str) {
    for attr in attrs {
        let Some(ident) = attr.path().get_ident() else {
            continue;
        };
        let name = ident.to_string();
        if BARE_SALSA_ATTR_NAMES.contains(&name.as_str()) {
            panic!("{rel_path}: {item_desc} has bare `#[{name}]`; spell it `#[salsa::{name}]`");
        }
    }
}

fn collect_tracked_struct(item_struct: &ItemStruct, rel_path: &str, inventory: &mut Inventory) {
    let Some((kind, attr)) = item_struct
        .attrs
        .iter()
        .find_map(|attr| Some((salsa_attr_kind(attr)?, attr)))
    else {
        return;
    };

    let options = parse_salsa_metas(attr)
        .iter()
        .map(render_meta)
        .collect::<Vec<_>>();
    let header = format!(
        "struct {}{}{}",
        item_struct.ident,
        normalize_tokens(&item_struct.generics),
        render_options_suffix(&options)
    );
    let fields = render_struct_fields(&item_struct.fields);
    let entry = StructEntry {
        rel_path: rel_path.to_string(),
        header,
        fields,
    };

    match kind {
        SalsaKind::Input => inventory.inputs.push(entry),
        SalsaKind::Interned => inventory.interned.push(entry),
        SalsaKind::Tracked => inventory.tracked_structs.push(entry),
    }
}

fn render_struct_fields(fields: &Fields) -> Vec<String> {
    let Fields::Named(named) = fields else {
        panic!("expected named fields on salsa struct");
    };
    named.named.iter().map(render_field).collect()
}

const FIELD_SALSA_ATTRS: [&str; 5] = ["tracked", "no_eq", "returns", "id", "default"];

fn render_field(field: &Field) -> String {
    let Some(ident) = &field.ident else {
        panic!("expected named field");
    };
    let ty = normalize_tokens(&field.ty);
    let options = field_salsa_options(field);
    format!("  {ident}: {ty}{}", render_options_suffix(&options))
}

fn field_salsa_options(field: &Field) -> Vec<String> {
    field
        .attrs
        .iter()
        .filter(|attr| {
            let Some(ident) = attr.path().get_ident() else {
                return false;
            };
            let name = ident.to_string();
            FIELD_SALSA_ATTRS.contains(&name.as_str())
        })
        .map(|attr| render_meta(&attr.meta))
        .collect()
}

fn collect_tracked_fn(
    attrs: &[Attribute],
    sig: &Signature,
    self_ty: Option<&str>,
    rel_path: &str,
    inventory: &mut Inventory,
) {
    let Some(attr) = attrs
        .iter()
        .find(|attr| matches!(salsa_attr_kind(attr), Some(SalsaKind::Tracked)))
    else {
        return;
    };

    let metas = parse_salsa_metas(attr);
    let options = metas.iter().map(render_meta).collect::<Vec<_>>();
    let cycle_recovery = cycle_recovery_of(&metas);
    let qualified_name = match self_ty {
        Some(self_ty) => format!("{self_ty}::{}", sig.ident),
        None => sig.ident.to_string(),
    };
    let rendered_signature = format!(
        "fn {qualified_name}({}){}{}",
        render_params(sig),
        render_return(sig),
        render_options_suffix(&options)
    );

    inventory.queries.push(QueryEntry {
        rel_path: rel_path.to_string(),
        rendered_signature,
        qualified_name,
        cycle_recovery,
    });
}

fn render_params(sig: &Signature) -> String {
    sig.inputs
        .iter()
        .map(render_fn_arg)
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_fn_arg(arg: &FnArg) -> String {
    match arg {
        FnArg::Receiver(_) => "self".to_string(),
        FnArg::Typed(pat_type) => {
            let name = normalize_tokens(pat_type.pat.as_ref());
            let ty = normalize_tokens(pat_type.ty.as_ref());
            format!("{name}: {ty}")
        },
    }
}

fn render_return(sig: &Signature) -> String {
    match &sig.output {
        ReturnType::Default => String::new(),
        ReturnType::Type(_, ty) => format!(" -> {}", normalize_tokens(ty.as_ref())),
    }
}

fn salsa_attr_kind(attr: &Attribute) -> Option<SalsaKind> {
    let mut segments = attr.path().segments.iter().rev();
    let kind = match segments.next()?.ident.to_string().as_str() {
        "input" => SalsaKind::Input,
        "interned" => SalsaKind::Interned,
        "tracked" => SalsaKind::Tracked,
        _ => return None,
    };
    let crate_ident = segments.next()?.ident.to_string();
    if crate_ident != "salsa" && crate_ident != "salsa_macros" {
        return None;
    }
    Some(kind)
}

/// Bare Salsa attributes, such as `#[salsa::tracked]`, have no options.
fn parse_salsa_metas(attr: &Attribute) -> Vec<Meta> {
    let Meta::List(_) = &attr.meta else {
        return Vec::new();
    };
    match attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated) {
        Ok(metas) => metas.into_iter().collect(),
        Err(err) => panic!("failed to parse salsa attribute options: {err}"),
    }
}

/// Salsa's fixpoint cycle strategy pairs `cycle_fn` with `cycle_initial` instead of using a
/// single `cycle_result` handler. A lone `cycle_result` keeps rendering as its bare handler name
/// so existing snapshots do not churn.
fn cycle_recovery_of(metas: &[Meta]) -> Option<String> {
    let cycle_result = find_name_value(metas, "cycle_result");
    let cycle_fn = find_name_value(metas, "cycle_fn");
    let cycle_initial = find_name_value(metas, "cycle_initial");

    if cycle_fn.is_none() && cycle_initial.is_none() {
        return cycle_result;
    }

    let mut parts = Vec::new();
    if let Some(cycle_fn) = cycle_fn {
        parts.push(format!("cycle_fn = {cycle_fn}"));
    }
    if let Some(cycle_initial) = cycle_initial {
        parts.push(format!("cycle_initial = {cycle_initial}"));
    }
    if let Some(cycle_result) = cycle_result {
        parts.push(format!("cycle_result = {cycle_result}"));
    }
    Some(parts.join(", "))
}

fn find_name_value(metas: &[Meta], name: &str) -> Option<String> {
    metas.iter().find_map(|meta| {
        let Meta::NameValue(name_value) = meta else {
            return None;
        };
        if !name_value.path.is_ident(name) {
            return None;
        }
        Some(normalize_tokens(&name_value.value))
    })
}

fn render_meta(meta: &Meta) -> String {
    match meta {
        Meta::Path(path) => normalize_tokens(path),
        Meta::List(list) => format!(
            "{}({})",
            normalize_tokens(&list.path),
            normalize_tokens(&list.tokens)
        ),
        Meta::NameValue(name_value) => format!(
            "{} = {}",
            normalize_tokens(&name_value.path),
            normalize_tokens(&name_value.value)
        ),
    }
}

fn render_options_suffix(options: &[String]) -> String {
    if options.is_empty() {
        String::new()
    } else {
        format!(" [{}]", options.join(", "))
    }
}

/// Remove punctuation spacing from `ToTokens::to_token_stream()` so snapshots render `Vec<File>`, not `Vec < File >`.
fn normalize_tokens(tokens: impl ToTokens) -> String {
    let collapsed = tokens
        .to_token_stream()
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    let mut rendered = collapsed;
    for (pattern, replacement) in [
        (" ::", "::"),
        (":: ", "::"),
        (" ,", ","),
        (" >", ">"),
        (" )", ")"),
        (" <", "<"),
        ("< ", "<"),
        ("( ", "("),
        ("& ", "&"),
    ] {
        rendered = rendered.replace(pattern, replacement);
    }
    rendered
}

fn render_report(inventory: Inventory) -> String {
    [
        render_struct_section("inputs", inventory.inputs),
        render_struct_section("interned", inventory.interned),
        render_struct_section("tracked structs", inventory.tracked_structs),
        render_query_section(&inventory.queries),
        render_cycle_section(&inventory.queries),
    ]
    .join("\n\n")
}

fn render_struct_section(title: &str, mut entries: Vec<StructEntry>) -> String {
    entries.sort_by(|left, right| {
        (&left.rel_path, &left.header).cmp(&(&right.rel_path, &right.header))
    });

    let mut lines = vec![format!("== {title} ==")];
    for entry in &entries {
        lines.push(format!("{}: {}", entry.rel_path, entry.header));
        lines.extend(entry.fields.iter().cloned());
    }
    lines.join("\n")
}

fn render_query_section(queries: &[QueryEntry]) -> String {
    let mut entries: Vec<&QueryEntry> = queries.iter().collect();
    entries.sort_by(|left, right| {
        (&left.rel_path, &left.rendered_signature)
            .cmp(&(&right.rel_path, &right.rendered_signature))
    });

    let mut lines = vec!["== tracked queries ==".to_string()];
    for entry in entries {
        lines.push(format!("{}: {}", entry.rel_path, entry.rendered_signature));
    }
    lines.join("\n")
}

fn render_cycle_section(queries: &[QueryEntry]) -> String {
    let mut entries: Vec<(&str, String)> = queries
        .iter()
        .filter_map(|entry| {
            let cycle_recovery = entry.cycle_recovery.as_ref()?;
            Some((
                entry.rel_path.as_str(),
                format!("{} -> {cycle_recovery}", entry.qualified_name),
            ))
        })
        .collect();
    entries.sort();

    let mut lines = vec!["== cycle recovery ==".to_string()];
    for (rel_path, line) in entries {
        lines.push(format!("{rel_path}: {line}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests;
