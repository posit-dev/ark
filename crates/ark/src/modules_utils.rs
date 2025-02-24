use libr::SEXP;

#[harp::register]
pub unsafe extern "C-unwind" fn ark_node_poke_cdr(node: SEXP, cdr: SEXP) -> anyhow::Result<SEXP> {
    libr::SETCDR(node, cdr);
    return Ok(harp::r_null());
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_deep_sleep(secs: SEXP) -> anyhow::Result<SEXP> {
    let secs = libr::Rf_asInteger(secs);
    let secs = std::time::Duration::from_secs(secs as u64);
    std::thread::sleep(secs);

    return Ok(harp::r_null());
}
