use anyhow::anyhow;
use libr::R_PreserveObject;
use libr::Rf_eval;
use libr::SEXP;
use rust_embed::RustEmbed;

use crate::call::RCall;
use crate::environment::R_ENVS;
use crate::exec::top_level_exec;
use crate::r_symbol;

pub static mut HARP_ENV: Option<SEXP> = None;

// Largely copied from `module.rs` in the Ark crate

#[derive(RustEmbed)]
#[folder = "src/modules"]
struct HarpModuleAsset;

fn with_asset<T, F>(file: &str, f: F) -> anyhow::Result<()>
where
    T: RustEmbed,
    F: FnOnce(&str) -> anyhow::Result<()>,
{
    let asset = T::get(file).ok_or(anyhow!("can't open asset {file}"))?;
    let data = std::str::from_utf8(&asset.data)?;
    f(data)
}

pub fn init_modules() -> anyhow::Result<()> {
    let namespace = unsafe {
        let new_env_call = RCall::new(r_symbol!("new.env")).build();
        Rf_eval(new_env_call.sexp, R_ENVS.base)
    };

    unsafe {
        R_PreserveObject(namespace);
        HARP_ENV = Some(namespace);
    }

    // We don't have `safe_eval()` yet so source the init file manually
    with_asset::<HarpModuleAsset, _>("init.R", |source| {
        let exprs = harp::parse_exprs(source)?;
        unsafe {
            let source_call = RCall::new(r_symbol!("source"))
                .param("exprs", exprs)
                .param("local", namespace)
                .build();
            top_level_exec(|| Rf_eval(source_call.sexp, R_ENVS.base))?;
        }
        Ok(())
    })?;

    // It's alright to source the init file twice
    for file in HarpModuleAsset::iter() {
        with_asset::<HarpModuleAsset, _>(&file, |source| {
            Ok(harp::source_str_in(source, namespace)?)
        })?;
    }

    Ok(())
}
