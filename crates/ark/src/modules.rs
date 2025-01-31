//
// modules.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use anyhow::anyhow;
use harp::environment::Environment;
use harp::environment::R_ENVS;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::r_symbol;
use harp::utils::r_poke_option;
use harp::RObject;
use libr::Rf_ScalarLogical;
use libr::SEXP;
use once_cell::sync::Lazy;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "src/modules/positron"]
struct PositronModuleAsset;

#[derive(RustEmbed)]
#[folder = "src/modules/rstudio"]
struct RStudioModuleAsset;

fn source_asset<T: RustEmbed>(file: &str, fun: &str, env: SEXP) -> anyhow::Result<()> {
    with_asset::<T, _>(file, |source| {
        let exprs = harp::parse_exprs_with_srcrefs(source)?;
        RFunction::new("", fun).param("exprs", exprs).call_in(env)?;
        Ok(())
    })
}

fn with_asset<T, F>(file: &str, f: F) -> anyhow::Result<()>
where
    T: RustEmbed,
    F: FnOnce(&str) -> anyhow::Result<()>,
{
    let asset = T::get(file).ok_or(anyhow!("can't open asset {file}"))?;
    let data = std::str::from_utf8(&asset.data)?;
    f(data)
}

pub static ARK_ENVS: Lazy<ArkEnvs> = Lazy::new(|| {
    let positron_ns = harp::parse_eval(
        "environment(as.environment('tools:positron')$.ps.internal)",
        Default::default(),
    )
    .unwrap()
    .sexp;

    let rstudio_ns = harp::parse_eval(
        "as.environment('tools:rstudio')$.__rstudio_ns__.",
        Default::default(),
    )
    .unwrap()
    .sexp;

    ArkEnvs {
        positron_ns,
        rstudio_ns,
    }
});

// Silences diagnostics when called from `r_task()`. Should only be
// accessed from the R thread.
unsafe impl Send for ArkEnvs {}
unsafe impl Sync for ArkEnvs {}

pub struct ArkEnvs {
    pub positron_ns: SEXP,
    pub rstudio_ns: SEXP,
}

/// Returns positron namespace.
pub fn initialize() -> anyhow::Result<RObject> {
    // If we are `testing`, set the corresponding R level global option
    if stdext::IS_TESTING {
        r_poke_option_ark_testing()
    }

    // Create the private Positron namespace.
    let namespace = RFunction::new("base", "new.env")
        .param("parent", R_ENVS.base)
        .call()?;

    // Load initial utils into the namespace
    with_asset::<PositronModuleAsset, _>("init.R", |source| {
        Ok(harp::source_str_in(source, namespace.sexp)?)
    })?;

    // Lock the environment. It will be unlocked automatically when updating.
    // Needs to happen after the `r_source_in()` above. We don't lock the
    // bindings to make it easy to make updates by `source()`ing inside the
    // temporarily unlocked environment.
    Environment::view(namespace.sexp).lock(false);

    // Load the positron and rstudio namespaces and their exported functions
    for file in PositronModuleAsset::iter() {
        source_asset::<PositronModuleAsset>(&file, "import_positron", namespace.sexp)?;
    }
    for file in RStudioModuleAsset::iter() {
        source_asset::<RStudioModuleAsset>(&file, "import_rstudio", namespace.sexp)?;
    }

    // Create a directory watcher that reloads module files as they are changed.
    #[cfg(debug_assertions)]
    {
        use std::path::Path;

        use debug::*;

        let source = std::env!("CARGO_MANIFEST_DIR");
        let root = Path::new(&source).join("src").join("modules").to_path_buf();

        if root.exists() {
            // First reload all modules from source to reflect new changes that have
            // not been built into the binary yet.
            log::trace!("Loading R modules from sources via cargo manifest");
            import_directory(
                &root.join("positron"),
                RModuleSource::Positron,
                namespace.sexp,
            )?;
            import_directory(
                &root.join("rstudio"),
                debug::RModuleSource::RStudio,
                namespace.sexp,
            )?;

            // Spawn the watcher thread when R is idle so we don't try to access
            // the R API while R is starting up
            crate::r_task::spawn_idle(move || async {
                log::info!("Watching R modules from sources via cargo manifest");
                spawn_watcher_thread(root);
            });
        } else {
            log::error!("Can't find ark R modules from sources");
        }
    }

    return Ok(namespace);
}

#[cfg(debug_assertions)]
mod debug {
    use std::collections::HashMap;
    use std::path::Path;
    use std::path::PathBuf;
    use std::time::Duration;
    use std::time::SystemTime;

    use harp::exec::RFunction;
    use harp::exec::RFunctionExt;
    use libr::SEXP;
    use stdext::spawn;

    use crate::interface::RMain;
    use crate::r_task;

    pub fn spawn_watcher_thread(root: PathBuf) {
        spawn!("ark-modules-watcher", {
            move || {
                let mut watcher = RModuleWatcher::new(root);
                match watcher.watch() {
                    Ok(_) => {},
                    Err(err) => log::error!("[watcher] Error watching files: {err:?}"),
                }
            }
        });
    }

    // NOTE(kevin): We use a custom watcher implementation here to detect changes
    // to module files, and automatically source those files when they change.
    //
    // The intention here is to make it easy to iterate and develop R modules
    // within Positron; files are automatically sourced when they change and
    // so any changes should appear live within Positrion immediately.
    //
    // We can't use a crate like 'notify' here as the file watchers they try
    // to add seem to conflict with the ones added by VSCode; at least, that's
    // what I observed on macOS.
    pub struct RModuleWatcher {
        path: PathBuf,
        cache: HashMap<PathBuf, (SystemTime, RModuleSource)>,
    }

    #[derive(Copy, Clone)]
    pub enum RModuleSource {
        Positron,
        RStudio,
    }

    impl RModuleWatcher {
        pub fn new(path: PathBuf) -> Self {
            Self {
                path,
                cache: HashMap::new(),
            }
        }

        pub fn init(&mut self, path: PathBuf, src: RModuleSource) -> anyhow::Result<()> {
            let entries = std::fs::read_dir(path)?;

            for entry in entries.into_iter() {
                if let Ok(entry) = entry {
                    let path = entry.path();
                    let modified = path.metadata()?.modified()?;
                    self.cache.insert(path, (modified, src));
                }
            }

            Ok(())
        }

        pub fn watch(&mut self) -> anyhow::Result<()> {
            let positron = self.path.join("positron");
            let rstudio = self.path.join("rstudio");

            self.init(positron, RModuleSource::Positron)?;
            self.init(rstudio, RModuleSource::RStudio)?;

            // Start looking for changes
            loop {
                std::thread::sleep(Duration::from_secs(1));

                if let Err(err) = self.update() {
                    log::error!("[watcher] error detecting changes: {err:?}");
                }
            }
        }

        pub fn update(&mut self) -> anyhow::Result<()> {
            for (path, (old_modified, src)) in self.cache.iter_mut() {
                let new_modified = path.metadata()?.modified()?;
                if *old_modified == new_modified {
                    continue;
                }

                r_task(|| {
                    let r_main = RMain::get();
                    if let Err(err) =
                        import_file(&path, *src, r_main.positron_ns.as_ref().unwrap().sexp)
                    {
                        log::error!("{err:?}");
                    }
                });
                *old_modified = new_modified;
            }

            Ok(())
        }
    }

    pub fn import_directory(directory: &Path, src: RModuleSource, env: SEXP) -> anyhow::Result<()> {
        log::info!("Loading modules from directory: {}", directory.display());
        let entries = std::fs::read_dir(directory)?;

        for entry in entries {
            match entry {
                Ok(entry) => import_file(&entry.path(), src, env)?,
                Err(err) => log::error!("Can't load modules from file: {err:?}"),
            };
        }

        Ok(())
    }

    pub fn import_file(path: &PathBuf, src: RModuleSource, env: SEXP) -> anyhow::Result<()> {
        let fun = match src {
            RModuleSource::Positron => "import_positron_path",
            RModuleSource::RStudio => "import_rstudio_path",
        };

        if path.extension().is_some_and(|ext| ext == "R") {
            let path_string = path.display().to_string();
            RFunction::new("", fun)
                .param("path", path_string)
                .call_in(env)?;
        }
        Ok(())
    }
}

fn r_poke_option_ark_testing() {
    unsafe {
        let value = Rf_ScalarLogical(1);
        r_poke_option(r_symbol!("ark.testing"), value);
    }
}

#[cfg(test)]
mod tests {
    use harp::environment::Environment;
    use libr::CLOENV;

    use crate::r_task;

    fn get_namespace(exports: Environment, fun: &str) -> Environment {
        let fun = exports.find(fun).unwrap();
        let ns = unsafe { CLOENV(fun) };
        Environment::view(ns)
    }

    #[test]
    fn test_environments_are_locked() {
        r_task(|| {
            let positron_exports =
                harp::parse_eval_base("as.environment('tools:positron')").unwrap();
            let rstudio_exports = harp::parse_eval_base("as.environment('tools:rstudio')").unwrap();

            let positron_exports = Environment::new(positron_exports);
            let rstudio_exports = Environment::new(rstudio_exports);

            assert!(positron_exports.is_locked());
            assert!(rstudio_exports.is_locked());

            let positron_ns = get_namespace(positron_exports, ".ps.ark.version");
            let rstudio_ns = get_namespace(rstudio_exports, ".rs.api.versionInfo");

            assert!(positron_ns.is_locked());
            assert!(rstudio_ns.is_locked());
        })
    }
}
