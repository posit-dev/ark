use std::ops::Deref;

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

pub fn get_asset<T: RustEmbed>(file: &str) -> anyhow::Result<String> {
    let asset = T::get(&file).ok_or(anyhow!("can't open asset {file}"))?;

    let u8_slice = asset.data.deref();
    let str_slice = std::str::from_utf8(u8_slice)?;

    Ok(str_slice.to_owned())
}

pub fn init_modules() -> anyhow::Result<()> {
    let namespace = RFunction::new("base", "new.env")
        .param("parent", R_ENVS.base)
        .call()?;

    unsafe {
        HARP_ENV = namespace.sexp;
    }

    for file in HarpModuleAsset::iter() {
        let code = get_asset::<HarpModuleAsset>(&file)?;
        r_source_str_in(&code, namespace.sexp)?;
    }

    Ok(())
}
