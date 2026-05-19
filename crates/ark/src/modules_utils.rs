use libr::SEXP;

#[harp::register]
pub unsafe extern "C-unwind" fn ark_node_poke_cdr(node: SEXP, cdr: SEXP) -> anyhow::Result<SEXP> {
    libr::SETCDR(node, cdr);
    return Ok(harp::r_null());
}

#[harp::register]
pub unsafe extern "C-unwind" fn ark_is_debug_build() -> anyhow::Result<SEXP> {
    cfg_if::cfg_if! {
        if #[cfg(debug_assertions)] {
            Ok(libr::Rf_ScalarLogical(1))
        } else {
            Ok(libr::Rf_ScalarLogical(0))
        }
    }
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_deep_sleep(secs: SEXP) -> anyhow::Result<SEXP> {
    let secs = libr::Rf_asInteger(secs);
    let secs = std::time::Duration::from_secs(secs as u64);
    std::thread::sleep(secs);

    return Ok(harp::r_null());
}
