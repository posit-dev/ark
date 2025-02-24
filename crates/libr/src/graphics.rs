//
// graphics.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

// This file captures the device description for the different versions
// of R graphics engines that we support.

// Opaque struct idea comes from the Nomicon
// https://doc.rust-lang.org/nomicon/ffi.html?highlight=Opaque#representing-opaque-structs

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use crate::Rboolean;
use crate::SEXP;

// ---------------------------------------------------------------------------------------

// Opaque structs used with R API graphics functions
//
// Some example usage:
// - `GEcurrentDevice()` returns a `pGEDevDesc`.
// - `GEinitDisplayList()` takes a `pGEDevDesc`.
// - Our `DeviceCallbacks` are given `pDevDesc` and `pGEcontext` pointers.
//
// The key idea is that the opaque `pGEDevDesc` pointer returned by `GEcurrentDevice()` is
// cast to a more specific "versioned" pointer using one of the versioned structs further
// below (the version is supplied by `R_GE_getVersion()`). We can then extract the
// corresponding versioned `.dev` field, and modify that. This is implemented through
// the `with_device` macro.

#[repr(C)]
pub struct DevDesc {
    _data: [u8; 0],
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}
pub type pDevDesc = *mut DevDesc;

#[repr(C)]
pub struct GEDevDesc {
    _data: [u8; 0],
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}
pub type pGEDevDesc = *mut GEDevDesc;

#[repr(C)]
pub struct R_GE_gcontext {
    _data: [u8; 0],
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}
pub type pGEcontext = *mut R_GE_gcontext;

// ---------------------------------------------------------------------------------------

// Supporting oqaque structs

// Used as `*mut GESystemDesc` by the `GEDevDesc` variants
#[repr(C)]
pub struct GESystemDesc {
    _data: [u8; 0],
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}

// ---------------------------------------------------------------------------------------

// Per-version variants of `GEDevDesc`.
// This has been mostly stable over the past 17 years, with the exception of adding
// `appending` around R 4.2.x. We need the actual struct layout, because we have to
// extract the `dev` field and set the `displayListOn` field.

// Graphics Engine Wrapper version 13 (R 4.0.x)
#[repr(C)]
pub struct GEDevDescVersion13 {
    pub dev: pDevDescVersion13,
    pub displayListOn: Rboolean,
    pub displayList: SEXP,
    pub DLlastElt: SEXP,
    pub savedSnapshot: SEXP,
    pub dirty: Rboolean,
    pub recordGraphics: Rboolean,
    pub gesd: [*mut GESystemDesc; 24usize],
    pub ask: Rboolean,
}

// Graphics Engine Wrapper version 14 (R 4.1.x)
#[repr(C)]
pub struct GEDevDescVersion14 {
    pub dev: pDevDescVersion14,
    pub displayListOn: Rboolean,
    pub displayList: SEXP,
    pub DLlastElt: SEXP,
    pub savedSnapshot: SEXP,
    pub dirty: Rboolean,
    pub recordGraphics: Rboolean,
    pub gesd: [*mut GESystemDesc; 24usize],
    pub ask: Rboolean,
}

// Graphics Engine Wrapper version 15 (R 4.2.x)
#[repr(C)]
pub struct GEDevDescVersion15 {
    pub dev: pDevDescVersion15,
    pub displayListOn: Rboolean,
    pub displayList: SEXP,
    pub DLlastElt: SEXP,
    pub savedSnapshot: SEXP,
    pub dirty: Rboolean,
    pub recordGraphics: Rboolean,
    pub gesd: [*mut GESystemDesc; 24usize],
    pub ask: Rboolean,
    pub appending: Rboolean,
}

// Graphics Engine Wrapper version 16 (R 4.3.0)
#[repr(C)]
pub struct GEDevDescVersion16 {
    pub dev: pDevDescVersion16,
    pub displayListOn: Rboolean,
    pub displayList: SEXP,
    pub DLlastElt: SEXP,
    pub savedSnapshot: SEXP,
    pub dirty: Rboolean,
    pub recordGraphics: Rboolean,
    pub gesd: [*mut GESystemDesc; 24usize],
    pub ask: Rboolean,
    pub appending: Rboolean,
}

// ---------------------------------------------------------------------------------------

// Per-version variants of `DevDesc`.
// This is subject to a large amount of change between R versions.

// Graphics Engine version 13 (R 4.0.x)
#[repr(C)]
pub struct DevDescVersion13 {
    pub left: f64,
    pub right: f64,
    pub bottom: f64,
    pub top: f64,
    pub clipLeft: f64,
    pub clipRight: f64,
    pub clipBottom: f64,
    pub clipTop: f64,
    pub xCharOffset: f64,
    pub yCharOffset: f64,
    pub yLineBias: f64,
    pub ipr: [f64; 2usize],
    pub cra: [f64; 2usize],
    pub gamma: f64,
    pub canClip: Rboolean,
    pub canChangeGamma: Rboolean,
    pub canHAdj: std::ffi::c_int,
    pub startps: f64,
    pub startcol: std::ffi::c_int,
    pub startfill: std::ffi::c_int,
    pub startlty: std::ffi::c_int,
    pub startfont: std::ffi::c_int,
    pub startgamma: f64,
    pub deviceSpecific: *mut std::ffi::c_void,
    pub displayListOn: Rboolean,
    pub canGenMouseDown: Rboolean,
    pub canGenMouseMove: Rboolean,
    pub canGenMouseUp: Rboolean,
    pub canGenKeybd: Rboolean,
    pub canGenIdle: Rboolean,
    pub gettingEvent: Rboolean,
    pub activate: Option<unsafe extern "C-unwind" fn(arg1: pDevDesc)>,
    pub circle:
        Option<unsafe extern "C-unwind" fn(x: f64, y: f64, r: f64, gc: pGEcontext, dd: pDevDesc)>,
    pub clip: Option<unsafe extern "C-unwind" fn(x0: f64, x1: f64, y0: f64, y1: f64, dd: pDevDesc)>,
    pub close: Option<unsafe extern "C-unwind" fn(dd: pDevDesc)>,
    pub deactivate: Option<unsafe extern "C-unwind" fn(arg1: pDevDesc)>,
    pub locator:
        Option<unsafe extern "C-unwind" fn(x: *mut f64, y: *mut f64, dd: pDevDesc) -> Rboolean>,
    pub line: Option<
        unsafe extern "C-unwind" fn(
            x1: f64,
            y1: f64,
            x2: f64,
            y2: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub metricInfo: Option<
        unsafe extern "C-unwind" fn(
            c: std::ffi::c_int,
            gc: pGEcontext,
            ascent: *mut f64,
            descent: *mut f64,
            width: *mut f64,
            dd: pDevDesc,
        ),
    >,
    pub mode: Option<unsafe extern "C-unwind" fn(mode: std::ffi::c_int, dd: pDevDesc)>,
    pub newPage: Option<unsafe extern "C-unwind" fn(gc: pGEcontext, dd: pDevDesc)>,
    pub polygon: Option<
        unsafe extern "C-unwind" fn(
            n: std::ffi::c_int,
            x: *mut f64,
            y: *mut f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub polyline: Option<
        unsafe extern "C-unwind" fn(
            n: std::ffi::c_int,
            x: *mut f64,
            y: *mut f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub rect: Option<
        unsafe extern "C-unwind" fn(
            x0: f64,
            y0: f64,
            x1: f64,
            y1: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub path: Option<
        unsafe extern "C-unwind" fn(
            x: *mut f64,
            y: *mut f64,
            npoly: std::ffi::c_int,
            nper: *mut std::ffi::c_int,
            winding: Rboolean,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub raster: Option<
        unsafe extern "C-unwind" fn(
            raster: *mut std::ffi::c_uint,
            w: std::ffi::c_int,
            h: std::ffi::c_int,
            x: f64,
            y: f64,
            width: f64,
            height: f64,
            rot: f64,
            interpolate: Rboolean,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub cap: Option<unsafe extern "C-unwind" fn(dd: pDevDesc) -> SEXP>,
    pub size: Option<
        unsafe extern "C-unwind" fn(
            left: *mut f64,
            right: *mut f64,
            bottom: *mut f64,
            top: *mut f64,
            dd: pDevDesc,
        ),
    >,
    pub strWidth: Option<
        unsafe extern "C-unwind" fn(
            str: *const std::ffi::c_char,
            gc: pGEcontext,
            dd: pDevDesc,
        ) -> f64,
    >,
    pub text: Option<
        unsafe extern "C-unwind" fn(
            x: f64,
            y: f64,
            str: *const std::ffi::c_char,
            rot: f64,
            hadj: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub onExit: Option<unsafe extern "C-unwind" fn(dd: pDevDesc)>,
    pub getEvent:
        Option<unsafe extern "C-unwind" fn(arg1: SEXP, arg2: *const std::ffi::c_char) -> SEXP>,
    pub newFrameConfirm: Option<unsafe extern "C-unwind" fn(dd: pDevDesc) -> Rboolean>,
    pub hasTextUTF8: Rboolean,
    pub textUTF8: Option<
        unsafe extern "C-unwind" fn(
            x: f64,
            y: f64,
            str: *const std::ffi::c_char,
            rot: f64,
            hadj: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub strWidthUTF8: Option<
        unsafe extern "C-unwind" fn(
            str: *const std::ffi::c_char,
            gc: pGEcontext,
            dd: pDevDesc,
        ) -> f64,
    >,
    pub wantSymbolUTF8: Rboolean,
    pub useRotatedTextInContour: Rboolean,
    pub eventEnv: SEXP,
    pub eventHelper: Option<unsafe extern "C-unwind" fn(dd: pDevDesc, code: std::ffi::c_int)>,
    pub holdflush: Option<
        unsafe extern "C-unwind" fn(dd: pDevDesc, level: std::ffi::c_int) -> std::ffi::c_int,
    >,
    pub haveTransparency: std::ffi::c_int,
    pub haveTransparentBg: std::ffi::c_int,
    pub haveRaster: std::ffi::c_int,
    pub haveCapture: std::ffi::c_int,
    pub haveLocator: std::ffi::c_int,
    pub reserved: [std::ffi::c_char; 64usize],
}
pub type pDevDescVersion13 = *mut DevDescVersion13;

// Graphics Engine version 14 (R 4.1.x)
#[repr(C)]
pub struct DevDescVersion14 {
    pub left: f64,
    pub right: f64,
    pub bottom: f64,
    pub top: f64,
    pub clipLeft: f64,
    pub clipRight: f64,
    pub clipBottom: f64,
    pub clipTop: f64,
    pub xCharOffset: f64,
    pub yCharOffset: f64,
    pub yLineBias: f64,
    pub ipr: [f64; 2usize],
    pub cra: [f64; 2usize],
    pub gamma: f64,
    pub canClip: Rboolean,
    pub canChangeGamma: Rboolean,
    pub canHAdj: std::ffi::c_int,
    pub startps: f64,
    pub startcol: std::ffi::c_int,
    pub startfill: std::ffi::c_int,
    pub startlty: std::ffi::c_int,
    pub startfont: std::ffi::c_int,
    pub startgamma: f64,
    pub deviceSpecific: *mut std::ffi::c_void,
    pub displayListOn: Rboolean,
    pub canGenMouseDown: Rboolean,
    pub canGenMouseMove: Rboolean,
    pub canGenMouseUp: Rboolean,
    pub canGenKeybd: Rboolean,
    pub canGenIdle: Rboolean,
    pub gettingEvent: Rboolean,
    pub activate: Option<unsafe extern "C-unwind" fn(arg1: pDevDesc)>,
    pub circle:
        Option<unsafe extern "C-unwind" fn(x: f64, y: f64, r: f64, gc: pGEcontext, dd: pDevDesc)>,
    pub clip: Option<unsafe extern "C-unwind" fn(x0: f64, x1: f64, y0: f64, y1: f64, dd: pDevDesc)>,
    pub close: Option<unsafe extern "C-unwind" fn(dd: pDevDesc)>,
    pub deactivate: Option<unsafe extern "C-unwind" fn(arg1: pDevDesc)>,
    pub locator:
        Option<unsafe extern "C-unwind" fn(x: *mut f64, y: *mut f64, dd: pDevDesc) -> Rboolean>,
    pub line: Option<
        unsafe extern "C-unwind" fn(
            x1: f64,
            y1: f64,
            x2: f64,
            y2: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub metricInfo: Option<
        unsafe extern "C-unwind" fn(
            c: std::ffi::c_int,
            gc: pGEcontext,
            ascent: *mut f64,
            descent: *mut f64,
            width: *mut f64,
            dd: pDevDesc,
        ),
    >,
    pub mode: Option<unsafe extern "C-unwind" fn(mode: std::ffi::c_int, dd: pDevDesc)>,
    pub newPage: Option<unsafe extern "C-unwind" fn(gc: pGEcontext, dd: pDevDesc)>,
    pub polygon: Option<
        unsafe extern "C-unwind" fn(
            n: std::ffi::c_int,
            x: *mut f64,
            y: *mut f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub polyline: Option<
        unsafe extern "C-unwind" fn(
            n: std::ffi::c_int,
            x: *mut f64,
            y: *mut f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub rect: Option<
        unsafe extern "C-unwind" fn(
            x0: f64,
            y0: f64,
            x1: f64,
            y1: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub path: Option<
        unsafe extern "C-unwind" fn(
            x: *mut f64,
            y: *mut f64,
            npoly: std::ffi::c_int,
            nper: *mut std::ffi::c_int,
            winding: Rboolean,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub raster: Option<
        unsafe extern "C-unwind" fn(
            raster: *mut std::ffi::c_uint,
            w: std::ffi::c_int,
            h: std::ffi::c_int,
            x: f64,
            y: f64,
            width: f64,
            height: f64,
            rot: f64,
            interpolate: Rboolean,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub cap: Option<unsafe extern "C-unwind" fn(dd: pDevDesc) -> SEXP>,
    pub size: Option<
        unsafe extern "C-unwind" fn(
            left: *mut f64,
            right: *mut f64,
            bottom: *mut f64,
            top: *mut f64,
            dd: pDevDesc,
        ),
    >,
    pub strWidth: Option<
        unsafe extern "C-unwind" fn(
            str: *const std::ffi::c_char,
            gc: pGEcontext,
            dd: pDevDesc,
        ) -> f64,
    >,
    pub text: Option<
        unsafe extern "C-unwind" fn(
            x: f64,
            y: f64,
            str: *const std::ffi::c_char,
            rot: f64,
            hadj: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub onExit: Option<unsafe extern "C-unwind" fn(dd: pDevDesc)>,
    pub getEvent:
        Option<unsafe extern "C-unwind" fn(arg1: SEXP, arg2: *const std::ffi::c_char) -> SEXP>,
    pub newFrameConfirm: Option<unsafe extern "C-unwind" fn(dd: pDevDesc) -> Rboolean>,
    pub hasTextUTF8: Rboolean,
    pub textUTF8: Option<
        unsafe extern "C-unwind" fn(
            x: f64,
            y: f64,
            str: *const std::ffi::c_char,
            rot: f64,
            hadj: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub strWidthUTF8: Option<
        unsafe extern "C-unwind" fn(
            str: *const std::ffi::c_char,
            gc: pGEcontext,
            dd: pDevDesc,
        ) -> f64,
    >,
    pub wantSymbolUTF8: Rboolean,
    pub useRotatedTextInContour: Rboolean,
    pub eventEnv: SEXP,
    pub eventHelper: Option<unsafe extern "C-unwind" fn(dd: pDevDesc, code: std::ffi::c_int)>,
    pub holdflush: Option<
        unsafe extern "C-unwind" fn(dd: pDevDesc, level: std::ffi::c_int) -> std::ffi::c_int,
    >,
    pub haveTransparency: std::ffi::c_int,
    pub haveTransparentBg: std::ffi::c_int,
    pub haveRaster: std::ffi::c_int,
    pub haveCapture: std::ffi::c_int,
    pub haveLocator: std::ffi::c_int,
    pub setPattern: Option<unsafe extern "C-unwind" fn(pattern: SEXP, dd: pDevDesc) -> SEXP>,
    pub releasePattern: Option<unsafe extern "C-unwind" fn(ref_: SEXP, dd: pDevDesc)>,
    pub setClipPath:
        Option<unsafe extern "C-unwind" fn(path: SEXP, ref_: SEXP, dd: pDevDesc) -> SEXP>,
    pub releaseClipPath: Option<unsafe extern "C-unwind" fn(ref_: SEXP, dd: pDevDesc)>,
    pub setMask: Option<unsafe extern "C-unwind" fn(path: SEXP, ref_: SEXP, dd: pDevDesc) -> SEXP>,
    pub releaseMask: Option<unsafe extern "C-unwind" fn(ref_: SEXP, dd: pDevDesc)>,
    pub deviceVersion: std::ffi::c_int,
    pub deviceClip: Rboolean,
    pub reserved: [std::ffi::c_char; 64usize],
}
pub type pDevDescVersion14 = *mut DevDescVersion14;

// Graphics Engine version 15 (R 4.2.x)
#[repr(C)]
pub struct DevDescVersion15 {
    pub left: f64,
    pub right: f64,
    pub bottom: f64,
    pub top: f64,
    pub clipLeft: f64,
    pub clipRight: f64,
    pub clipBottom: f64,
    pub clipTop: f64,
    pub xCharOffset: f64,
    pub yCharOffset: f64,
    pub yLineBias: f64,
    pub ipr: [f64; 2usize],
    pub cra: [f64; 2usize],
    pub gamma: f64,
    pub canClip: Rboolean,
    pub canChangeGamma: Rboolean,
    pub canHAdj: std::ffi::c_int,
    pub startps: f64,
    pub startcol: std::ffi::c_int,
    pub startfill: std::ffi::c_int,
    pub startlty: std::ffi::c_int,
    pub startfont: std::ffi::c_int,
    pub startgamma: f64,
    pub deviceSpecific: *mut std::ffi::c_void,
    pub displayListOn: Rboolean,
    pub canGenMouseDown: Rboolean,
    pub canGenMouseMove: Rboolean,
    pub canGenMouseUp: Rboolean,
    pub canGenKeybd: Rboolean,
    pub canGenIdle: Rboolean,
    pub gettingEvent: Rboolean,
    pub activate: Option<unsafe extern "C-unwind" fn(arg1: pDevDesc)>,
    pub circle:
        Option<unsafe extern "C-unwind" fn(x: f64, y: f64, r: f64, gc: pGEcontext, dd: pDevDesc)>,
    pub clip: Option<unsafe extern "C-unwind" fn(x0: f64, x1: f64, y0: f64, y1: f64, dd: pDevDesc)>,
    pub close: Option<unsafe extern "C-unwind" fn(dd: pDevDesc)>,
    pub deactivate: Option<unsafe extern "C-unwind" fn(arg1: pDevDesc)>,
    pub locator:
        Option<unsafe extern "C-unwind" fn(x: *mut f64, y: *mut f64, dd: pDevDesc) -> Rboolean>,
    pub line: Option<
        unsafe extern "C-unwind" fn(
            x1: f64,
            y1: f64,
            x2: f64,
            y2: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub metricInfo: Option<
        unsafe extern "C-unwind" fn(
            c: std::ffi::c_int,
            gc: pGEcontext,
            ascent: *mut f64,
            descent: *mut f64,
            width: *mut f64,
            dd: pDevDesc,
        ),
    >,
    pub mode: Option<unsafe extern "C-unwind" fn(mode: std::ffi::c_int, dd: pDevDesc)>,
    pub newPage: Option<unsafe extern "C-unwind" fn(gc: pGEcontext, dd: pDevDesc)>,
    pub polygon: Option<
        unsafe extern "C-unwind" fn(
            n: std::ffi::c_int,
            x: *mut f64,
            y: *mut f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub polyline: Option<
        unsafe extern "C-unwind" fn(
            n: std::ffi::c_int,
            x: *mut f64,
            y: *mut f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub rect: Option<
        unsafe extern "C-unwind" fn(
            x0: f64,
            y0: f64,
            x1: f64,
            y1: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub path: Option<
        unsafe extern "C-unwind" fn(
            x: *mut f64,
            y: *mut f64,
            npoly: std::ffi::c_int,
            nper: *mut std::ffi::c_int,
            winding: Rboolean,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub raster: Option<
        unsafe extern "C-unwind" fn(
            raster: *mut std::ffi::c_uint,
            w: std::ffi::c_int,
            h: std::ffi::c_int,
            x: f64,
            y: f64,
            width: f64,
            height: f64,
            rot: f64,
            interpolate: Rboolean,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub cap: Option<unsafe extern "C-unwind" fn(dd: pDevDesc) -> SEXP>,
    pub size: Option<
        unsafe extern "C-unwind" fn(
            left: *mut f64,
            right: *mut f64,
            bottom: *mut f64,
            top: *mut f64,
            dd: pDevDesc,
        ),
    >,
    pub strWidth: Option<
        unsafe extern "C-unwind" fn(
            str: *const std::ffi::c_char,
            gc: pGEcontext,
            dd: pDevDesc,
        ) -> f64,
    >,
    pub text: Option<
        unsafe extern "C-unwind" fn(
            x: f64,
            y: f64,
            str: *const std::ffi::c_char,
            rot: f64,
            hadj: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub onExit: Option<unsafe extern "C-unwind" fn(dd: pDevDesc)>,
    pub getEvent:
        Option<unsafe extern "C-unwind" fn(arg1: SEXP, arg2: *const std::ffi::c_char) -> SEXP>,
    pub newFrameConfirm: Option<unsafe extern "C-unwind" fn(dd: pDevDesc) -> Rboolean>,
    pub hasTextUTF8: Rboolean,
    pub textUTF8: Option<
        unsafe extern "C-unwind" fn(
            x: f64,
            y: f64,
            str: *const std::ffi::c_char,
            rot: f64,
            hadj: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub strWidthUTF8: Option<
        unsafe extern "C-unwind" fn(
            str: *const std::ffi::c_char,
            gc: pGEcontext,
            dd: pDevDesc,
        ) -> f64,
    >,
    pub wantSymbolUTF8: Rboolean,
    pub useRotatedTextInContour: Rboolean,
    pub eventEnv: SEXP,
    pub eventHelper: Option<unsafe extern "C-unwind" fn(dd: pDevDesc, code: std::ffi::c_int)>,
    pub holdflush: Option<
        unsafe extern "C-unwind" fn(dd: pDevDesc, level: std::ffi::c_int) -> std::ffi::c_int,
    >,
    pub haveTransparency: std::ffi::c_int,
    pub haveTransparentBg: std::ffi::c_int,
    pub haveRaster: std::ffi::c_int,
    pub haveCapture: std::ffi::c_int,
    pub haveLocator: std::ffi::c_int,
    pub setPattern: Option<unsafe extern "C-unwind" fn(pattern: SEXP, dd: pDevDesc) -> SEXP>,
    pub releasePattern: Option<unsafe extern "C-unwind" fn(ref_: SEXP, dd: pDevDesc)>,
    pub setClipPath:
        Option<unsafe extern "C-unwind" fn(path: SEXP, ref_: SEXP, dd: pDevDesc) -> SEXP>,
    pub releaseClipPath: Option<unsafe extern "C-unwind" fn(ref_: SEXP, dd: pDevDesc)>,
    pub setMask: Option<unsafe extern "C-unwind" fn(path: SEXP, ref_: SEXP, dd: pDevDesc) -> SEXP>,
    pub releaseMask: Option<unsafe extern "C-unwind" fn(ref_: SEXP, dd: pDevDesc)>,
    pub deviceVersion: std::ffi::c_int,
    pub deviceClip: Rboolean,
    pub defineGroup: Option<
        unsafe extern "C-unwind" fn(
            source: SEXP,
            op: std::ffi::c_int,
            destination: SEXP,
            dd: pDevDesc,
        ) -> SEXP,
    >,
    pub useGroup: Option<unsafe extern "C-unwind" fn(ref_: SEXP, trans: SEXP, dd: pDevDesc)>,
    pub releaseGroup: Option<unsafe extern "C-unwind" fn(ref_: SEXP, dd: pDevDesc)>,
    pub stroke: Option<unsafe extern "C-unwind" fn(path: SEXP, gc: pGEcontext, dd: pDevDesc)>,
    pub fill: Option<
        unsafe extern "C-unwind" fn(
            path: SEXP,
            rule: std::ffi::c_int,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub fillStroke: Option<
        unsafe extern "C-unwind" fn(
            path: SEXP,
            rule: std::ffi::c_int,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub capabilities: Option<unsafe extern "C-unwind" fn(cap: SEXP) -> SEXP>,
    pub reserved: [std::ffi::c_char; 64usize],
}
pub type pDevDescVersion15 = *mut DevDescVersion15;

// Graphics Engine version 16 (R 4.3.0)
#[repr(C)]
pub struct DevDescVersion16 {
    pub left: f64,
    pub right: f64,
    pub bottom: f64,
    pub top: f64,
    pub clipLeft: f64,
    pub clipRight: f64,
    pub clipBottom: f64,
    pub clipTop: f64,
    pub xCharOffset: f64,
    pub yCharOffset: f64,
    pub yLineBias: f64,
    pub ipr: [f64; 2usize],
    pub cra: [f64; 2usize],
    pub gamma: f64,
    pub canClip: Rboolean,
    pub canChangeGamma: Rboolean,
    pub canHAdj: std::ffi::c_int,
    pub startps: f64,
    pub startcol: std::ffi::c_int,
    pub startfill: std::ffi::c_int,
    pub startlty: std::ffi::c_int,
    pub startfont: std::ffi::c_int,
    pub startgamma: f64,
    pub deviceSpecific: *mut std::ffi::c_void,
    pub displayListOn: Rboolean,
    pub canGenMouseDown: Rboolean,
    pub canGenMouseMove: Rboolean,
    pub canGenMouseUp: Rboolean,
    pub canGenKeybd: Rboolean,
    pub canGenIdle: Rboolean,
    pub gettingEvent: Rboolean,
    pub activate: Option<unsafe extern "C-unwind" fn(arg1: pDevDesc)>,
    pub circle:
        Option<unsafe extern "C-unwind" fn(x: f64, y: f64, r: f64, gc: pGEcontext, dd: pDevDesc)>,
    pub clip: Option<unsafe extern "C-unwind" fn(x0: f64, x1: f64, y0: f64, y1: f64, dd: pDevDesc)>,
    pub close: Option<unsafe extern "C-unwind" fn(dd: pDevDesc)>,
    pub deactivate: Option<unsafe extern "C-unwind" fn(arg1: pDevDesc)>,
    pub locator:
        Option<unsafe extern "C-unwind" fn(x: *mut f64, y: *mut f64, dd: pDevDesc) -> Rboolean>,
    pub line: Option<
        unsafe extern "C-unwind" fn(
            x1: f64,
            y1: f64,
            x2: f64,
            y2: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub metricInfo: Option<
        unsafe extern "C-unwind" fn(
            c: std::ffi::c_int,
            gc: pGEcontext,
            ascent: *mut f64,
            descent: *mut f64,
            width: *mut f64,
            dd: pDevDesc,
        ),
    >,
    pub mode: Option<unsafe extern "C-unwind" fn(mode: std::ffi::c_int, dd: pDevDesc)>,
    pub newPage: Option<unsafe extern "C-unwind" fn(gc: pGEcontext, dd: pDevDesc)>,
    pub polygon: Option<
        unsafe extern "C-unwind" fn(
            n: std::ffi::c_int,
            x: *mut f64,
            y: *mut f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub polyline: Option<
        unsafe extern "C-unwind" fn(
            n: std::ffi::c_int,
            x: *mut f64,
            y: *mut f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub rect: Option<
        unsafe extern "C-unwind" fn(
            x0: f64,
            y0: f64,
            x1: f64,
            y1: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub path: Option<
        unsafe extern "C-unwind" fn(
            x: *mut f64,
            y: *mut f64,
            npoly: std::ffi::c_int,
            nper: *mut std::ffi::c_int,
            winding: Rboolean,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub raster: Option<
        unsafe extern "C-unwind" fn(
            raster: *mut std::ffi::c_uint,
            w: std::ffi::c_int,
            h: std::ffi::c_int,
            x: f64,
            y: f64,
            width: f64,
            height: f64,
            rot: f64,
            interpolate: Rboolean,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub cap: Option<unsafe extern "C-unwind" fn(dd: pDevDesc) -> SEXP>,
    pub size: Option<
        unsafe extern "C-unwind" fn(
            left: *mut f64,
            right: *mut f64,
            bottom: *mut f64,
            top: *mut f64,
            dd: pDevDesc,
        ),
    >,
    pub strWidth: Option<
        unsafe extern "C-unwind" fn(
            str: *const std::ffi::c_char,
            gc: pGEcontext,
            dd: pDevDesc,
        ) -> f64,
    >,
    pub text: Option<
        unsafe extern "C-unwind" fn(
            x: f64,
            y: f64,
            str: *const std::ffi::c_char,
            rot: f64,
            hadj: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub onExit: Option<unsafe extern "C-unwind" fn(dd: pDevDesc)>,
    pub getEvent:
        Option<unsafe extern "C-unwind" fn(arg1: SEXP, arg2: *const std::ffi::c_char) -> SEXP>,
    pub newFrameConfirm: Option<unsafe extern "C-unwind" fn(dd: pDevDesc) -> Rboolean>,
    pub hasTextUTF8: Rboolean,
    pub textUTF8: Option<
        unsafe extern "C-unwind" fn(
            x: f64,
            y: f64,
            str: *const std::ffi::c_char,
            rot: f64,
            hadj: f64,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub strWidthUTF8: Option<
        unsafe extern "C-unwind" fn(
            str: *const std::ffi::c_char,
            gc: pGEcontext,
            dd: pDevDesc,
        ) -> f64,
    >,
    pub wantSymbolUTF8: Rboolean,
    pub useRotatedTextInContour: Rboolean,
    pub eventEnv: SEXP,
    pub eventHelper: Option<unsafe extern "C-unwind" fn(dd: pDevDesc, code: std::ffi::c_int)>,
    pub holdflush: Option<
        unsafe extern "C-unwind" fn(dd: pDevDesc, level: std::ffi::c_int) -> std::ffi::c_int,
    >,
    pub haveTransparency: std::ffi::c_int,
    pub haveTransparentBg: std::ffi::c_int,
    pub haveRaster: std::ffi::c_int,
    pub haveCapture: std::ffi::c_int,
    pub haveLocator: std::ffi::c_int,
    pub setPattern: Option<unsafe extern "C-unwind" fn(pattern: SEXP, dd: pDevDesc) -> SEXP>,
    pub releasePattern: Option<unsafe extern "C-unwind" fn(ref_: SEXP, dd: pDevDesc)>,
    pub setClipPath:
        Option<unsafe extern "C-unwind" fn(path: SEXP, ref_: SEXP, dd: pDevDesc) -> SEXP>,
    pub releaseClipPath: Option<unsafe extern "C-unwind" fn(ref_: SEXP, dd: pDevDesc)>,
    pub setMask: Option<unsafe extern "C-unwind" fn(path: SEXP, ref_: SEXP, dd: pDevDesc) -> SEXP>,
    pub releaseMask: Option<unsafe extern "C-unwind" fn(ref_: SEXP, dd: pDevDesc)>,
    pub deviceVersion: std::ffi::c_int,
    pub deviceClip: Rboolean,
    pub defineGroup: Option<
        unsafe extern "C-unwind" fn(
            source: SEXP,
            op: std::ffi::c_int,
            destination: SEXP,
            dd: pDevDesc,
        ) -> SEXP,
    >,
    pub useGroup: Option<unsafe extern "C-unwind" fn(ref_: SEXP, trans: SEXP, dd: pDevDesc)>,
    pub releaseGroup: Option<unsafe extern "C-unwind" fn(ref_: SEXP, dd: pDevDesc)>,
    pub stroke: Option<unsafe extern "C-unwind" fn(path: SEXP, gc: pGEcontext, dd: pDevDesc)>,
    pub fill: Option<
        unsafe extern "C-unwind" fn(
            path: SEXP,
            rule: std::ffi::c_int,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub fillStroke: Option<
        unsafe extern "C-unwind" fn(
            path: SEXP,
            rule: std::ffi::c_int,
            gc: pGEcontext,
            dd: pDevDesc,
        ),
    >,
    pub capabilities: Option<unsafe extern "C-unwind" fn(cap: SEXP) -> SEXP>,
    pub glyph: Option<
        unsafe extern "C-unwind" fn(
            n: std::ffi::c_int,
            glyphs: *mut std::ffi::c_int,
            x: *mut f64,
            y: *mut f64,
            font: SEXP,
            size: f64,
            colour: std::ffi::c_int,
            rot: f64,
            dd: pDevDesc,
        ),
    >,
    pub reserved: [std::ffi::c_char; 64usize],
}
pub type pDevDescVersion16 = *mut DevDescVersion16;
