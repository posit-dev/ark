use biome_rowan::TextRange;
use oak_db::Db;
use oak_db::Definition;
use oak_db::File;
use oak_db::Formal;
use oak_db::Identifier;
use oak_db::Package;
use oak_db::Resolution;

/// What [`callee_description()`] finds for a callee.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CalleeLookup<'db> {
    /// The callee's formals and where they come from, or `None` under the
    /// conditions listed on [`callee_description()`].
    pub description: Option<CalleeDescription<'db>>,
    /// The installed packages resolution consulted
    /// ([`Package::is_installed()`]), in lookup order, whether or not they
    /// supplied a definition. Pending sources of these packages can
    /// still supply the callee or change its formals, so a host reports them
    /// whether or not `description` is `Some`.
    pub packages: Vec<Package>,
}

/// A callee's formal parameters from its source definition, without R
/// evaluation. Defaults retain their source spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalleeDescription<'db> {
    pub formals: Vec<Formal>,
    /// The definitions the callee resolves to, in lookup order. Each has
    /// these formals.
    pub definitions: Vec<Definition<'db>>,
    /// The package whose files hold the first definition, whose documentation
    /// describes the formals. For a re-export, this is the package that
    /// defines the function, not the one the callee names. `None` for a
    /// function defined outside any package, such as in a script.
    pub package: Option<Package>,
}

/// Describe the callee whose expression spans `callee` in `file`, such as
/// `f` in `f(x)` or `pkg::f` in `pkg::f(x)`. This is the one place that
/// decides which definitions a callee resolves to.
///
/// A bare name resolves through [`File::resolution_at()`]. Qualified names
/// resolve through [`Package::resolve()`], respecting `::` export
/// visibility, and consult the named package's re-export chain
/// ([`Package::reexport_chain()`]). Primitives with function-definition
/// stubs in `base`'s files are described like other functions.
///
/// The description is `None` under any of these conditions.
///
/// - The callee is another kind of expression, such as `obj$f` or `f()`.
/// - Nothing reachable binds the name, or the package's sources aren't
///   available.
/// - A definition isn't a function definition. This includes `.Primitive()`
///   stubs, aliases such as `f <- g`, and wrapped functions such as
///   `f <- memoise(g)`.
/// - Several definitions reach the call and their formals differ, including
///   their defaults' source text. Choosing one could show the wrong formals.
pub fn callee_description<'db>(
    db: &'db dyn Db,
    file: File,
    callee: TextRange,
) -> CalleeLookup<'db> {
    // At the callee's end, `obj$f` classifies as a member and `f()` has no
    // name token. Neither has statically resolvable formals here.
    let Some(identifier) = Identifier::classify(db, file, callee.end()) else {
        return CalleeLookup::default();
    };
    let resolution = match identifier {
        // A variable must span the whole callee, not just a subexpression.
        Identifier::Variable { range, .. } if range == callee => {
            file.resolution_at(db, range.start())
        },
        Identifier::NamespaceAccess {
            package,
            name,
            visibility,
        } => {
            let Some(package) = db.package_by_name(package.text(db).as_str()) else {
                return CalleeLookup::default();
            };
            Resolution {
                definitions: package.resolve(db, name, visibility),
                packages: package
                    .reexport_chain(db, name.text(db))
                    .into_iter()
                    .filter(|package| package.is_installed(db))
                    .collect(),
            }
        },
        Identifier::Variable { .. } | Identifier::Member { .. } => {
            return CalleeLookup::default();
        },
    };

    let Resolution {
        definitions,
        packages,
    } = resolution;
    let description = agreed_formals(db, &definitions).map(|formals| CalleeDescription {
        formals,
        package: definitions
            .first()
            .and_then(|definition| definition.file(db).package(db)),
        definitions,
    });

    CalleeLookup {
        description,
        packages,
    }
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
