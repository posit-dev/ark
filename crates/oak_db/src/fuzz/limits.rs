//! Stable acceptance limits for saved scenarios.
//!
//! These are independent of generation budgets: reducing a mutation budget must
//! not make an existing failure artifact impossible to replay.

use crate::fuzz::scenario::Op;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::spec::is_identifier;
use crate::fuzz::traversal::count_statements;
use crate::fuzz::traversal::height;
use crate::fuzz::traversal::sited_programs;

// == Artifact acceptance limits ==

const MAX_FILES: usize = 5;
const MAX_OPS: usize = 16;
const MAX_DEPTH: usize = 3;
const MAX_PACKAGES: usize = 3;
const MAX_EXPORTS: usize = 3;
const MAX_REEXPORTS: usize = 3;
/// Package, export, re-export, and query names, measured in bytes.
const MAX_NAME: usize = 32;
/// Leave room for a compound insertion to exceed the statement growth budget.
const CEILING_STATEMENTS: usize = 28;
/// Rendered bytes per program, including edit replacements.
const CEILING_TEXT: usize = 4_000;

/// Apply stable replay limits to initial programs and edit replacements.
pub(super) fn within_bounds(scenario: &Scenario) -> anyhow::Result<()> {
    let files = scenario.initial.files.len();
    if files > MAX_FILES {
        return Err(anyhow::anyhow!(
            "the workspace has {files} files but replay accepts at most {MAX_FILES}"
        ));
    }

    let ops = scenario.ops.len();
    if ops > MAX_OPS {
        return Err(anyhow::anyhow!(
            "the history has {ops} operations but replay accepts at most {MAX_OPS}"
        ));
    }

    within_package_bounds(scenario)?;

    for (context, name) in sited_query_names(scenario) {
        within_name_bounds(name, &format!("{context} name"))?;
    }

    for (site, program) in sited_programs(scenario) {
        for stmt in &program.statements {
            let depth = height(stmt);
            if depth > MAX_DEPTH {
                return Err(anyhow::anyhow!(
                    "{} nests {depth} levels but replay accepts at most {MAX_DEPTH}",
                    site.render()
                ));
            }
        }

        let statements = count_statements(&program.statements);
        if statements > CEILING_STATEMENTS {
            return Err(anyhow::anyhow!(
                "{} has {statements} statements, over the {CEILING_STATEMENTS} ceiling",
                site.render()
            ));
        }

        let width = program.render().text.len();
        if width > CEILING_TEXT {
            return Err(anyhow::anyhow!(
                "{} renders {width} bytes, over the {CEILING_TEXT} ceiling",
                site.render()
            ));
        }
    }

    Ok(())
}

fn within_package_bounds(scenario: &Scenario) -> anyhow::Result<()> {
    let packages = scenario.initial.packages.len();
    if packages > MAX_PACKAGES {
        return Err(anyhow::anyhow!(
            "the workspace has {packages} packages but replay accepts at most {MAX_PACKAGES}"
        ));
    }

    for (index, package) in scenario.initial.packages.iter().enumerate() {
        let context = format!("package {index} name");
        within_name_bounds(&package.name, &context)?;
        within_identifier_charset(&package.name, &context)?;

        if package.exports.len() > MAX_EXPORTS {
            return Err(anyhow::anyhow!(
                "package {:?} has {} exports but replay accepts at most {MAX_EXPORTS}",
                package.name,
                package.exports.len()
            ));
        }
        for (export_index, export) in package.exports.iter().enumerate() {
            let context = format!("package {index} export {export_index}");
            within_name_bounds(export, &context)?;
            within_identifier_charset(export, &context)?;
        }

        if package.reexports.len() > MAX_REEXPORTS {
            return Err(anyhow::anyhow!(
                "package {:?} has {} reexports but replay accepts at most {MAX_REEXPORTS}",
                package.name,
                package.reexports.len()
            ));
        }
        for (reexport_index, reexport) in package.reexports.iter().enumerate() {
            let name_context = format!("package {index} reexport {reexport_index} name");
            within_name_bounds(&reexport.name, &name_context)?;
            within_identifier_charset(&reexport.name, &name_context)?;

            let from_context = format!("package {index} reexport {reexport_index} from");
            within_name_bounds(&reexport.from, &from_context)?;
            within_identifier_charset(&reexport.from, &from_context)?;
        }
    }

    Ok(())
}

fn within_name_bounds(name: &str, context: &str) -> anyhow::Result<()> {
    if name.len() > MAX_NAME {
        return Err(anyhow::anyhow!(
            "{context} is {} bytes, over the {MAX_NAME} byte limit",
            name.len()
        ));
    }
    Ok(())
}

fn within_identifier_charset(name: &str, context: &str) -> anyhow::Result<()> {
    if !is_identifier(name) {
        return Err(anyhow::anyhow!(
            "{context} {name:?} is not a valid identifier"
        ));
    }
    Ok(())
}

/// Query names go directly to `Name::new()` without parsing, so only their
/// size is restricted, not their spelling.
fn sited_query_names(scenario: &Scenario) -> Vec<(String, &str)> {
    let mut out = Vec::new();
    if let Some(name) = query_name(&scenario.cold_entry) {
        out.push(("cold_entry".to_string(), name));
    }
    for (index, op) in scenario.ops.iter().enumerate() {
        if let Op::Query(query) = op {
            if let Some(name) = query_name(query) {
                out.push((format!("op {index}"), name));
            }
        }
    }
    out
}

fn query_name(query: &Query) -> Option<&str> {
    match query {
        Query::Resolve(_, name) => Some(name.as_str()),
        Query::PackageResolve(_, name, _) => Some(name.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use oak_semantic::fuzz::Program;

    use super::*;
    use crate::fuzz::build::binding;
    use crate::fuzz::corpus;
    use crate::fuzz::scenario::Edit;
    use crate::fuzz::spec::FileId;

    /// Fixed artifact ceilings must not follow generation budget changes.
    #[test]
    fn test_replay_accepts_programs_above_the_growth_threshold() {
        let mut scenario = corpus::case("acyclic_pair_closes_then_reopens");
        let program = Program {
            statements: (0..28)
                .map(|index| binding(&format!("val_{index}")))
                .collect(),
        };
        scenario.initial.files[0].program = program.clone();
        scenario.ops.push(Op::Edit(Edit {
            file: FileId(0),
            program,
        }));
        let json = match scenario.to_json() {
            Ok(json) => json,
            Err(err) => panic!("serialization failed: {err:?}"),
        };
        assert!(Scenario::from_json(json.as_bytes()).is_ok());
    }
}
