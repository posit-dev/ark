use anyhow::anyhow;
use libr::SEXP;
use rust_embed::RustEmbed;

use crate::environment::R_ENVS;
use crate::exec::r_source_str_in;
use crate::exec::RFunction;
use crate::exec::RFunctionExt;

pub static mut HARP_ENV: SEXP = std::ptr::null_mut();

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
    let namespace = RFunction::new("base", "new.env")
        .param("parent", R_ENVS.base)
        .call()?;

    unsafe {
        HARP_ENV = namespace.sexp;
    }

    for file in HarpModuleAsset::iter() {
        with_asset::<HarpModuleAsset, _>(&file, |source| {
            Ok(r_source_str_in(source, namespace.sexp)?)
        })?;
    }

    Ok(())
}
