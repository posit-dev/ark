use biome_rowan::TextRange;
use oak_db::Db;
use oak_db::Definition;
use oak_db::File;
use oak_db::Formal;
use oak_db::Identifier;

/// A callee's formal parameters from its source definition, without R
/// evaluation. Defaults retain their source spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalleeDescription {
    pub formals: Vec<Formal>,
}

/// Describe the callee whose expression spans `callee` in `file`, such as
/// `f` in `f(x)` or `pkg::f` in `pkg::f(x)`.
///
/// A bare name resolves through [`File::resolve_at()`]. Qualified names
/// resolve through [`oak_db::Package::resolve()`], respecting `::` export
/// visibility. Primitives with function-definition stubs in `base`'s files
/// are described like other functions.
///
/// Returns `None` under any of these conditions.
///
/// - The callee is another kind of expression, such as `obj$f` or `f()`.
/// - Nothing reachable binds the name, or the package's sources aren't
///   available.
/// - A definition isn't a function definition. This includes `.Primitive()`
///   stubs, aliases such as `f <- g`, and wrapped functions such as
///   `f <- memoise(g)`.
/// - Several definitions reach the call and their formals differ, including
///   their defaults' source text. Choosing one could show the wrong formals.
pub fn callee_description(db: &dyn Db, file: File, callee: TextRange) -> Option<CalleeDescription> {
    // At the callee's end, `obj$f` classifies as a member and `f()` has no
    // name token. Neither has statically resolvable formals here.
    let definitions = match Identifier::classify(db, file, callee.end())? {
        // A variable must span the whole callee, not just a subexpression.
        Identifier::Variable { range, .. } if range == callee => file.resolve_at(db, range.start()),
        Identifier::NamespaceAccess {
            package,
            name,
            visibility,
        } => {
            let package = db.package_by_name(package.text(db).as_str())?;
            package.resolve(db, name, visibility)
        },
        Identifier::Variable { .. } | Identifier::Member { .. } => return None,
    };

    agreed_formals(db, &definitions).map(|formals| CalleeDescription { formals })
}

fn agreed_formals(db: &dyn Db, definitions: &[Definition]) -> Option<Vec<Formal>> {
    let (first, rest) = definitions.split_first()?;
    let formals = first.formals(db)?;

    for definition in rest {
        if definition.formals(db)? != formals {
            return None;
        }
    }

    Some(formals)
}
