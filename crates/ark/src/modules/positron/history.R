# Some background on how `.Rhistory` works, and why we need a
# `utils::timestamp()` override on Windows
#
# At the R level, R history is managed by 3 core functions:
# - `savehistory()` - Save C level history buffer to a `.Rhistory` file
# - `loadhistory()` - Load `.Rhistory` file into C level history buffer
# - `timestamp()` - Print timestamp and write it to C level history buffer
#
# There are two places where this history buffer is typically written to when
# running command line R:
# - R `timestamp()`
#   - Unix: `timestamp -> addhistory -> ptr_R_addhistory -> Rstd_addhistory -> GNU Readline's add_history`
#   - Windows: `timestamp -> addhistory -> wgl_histadd`
# - C `R_ReadConsole(addtohistory = 1)`
#   - Unix: `R_ReadConsole -> ptr_ReadConsole -> Rstd_ReadConsole -> GNU Readline's add_history`
#   - Windows: `R_ReadConsole -> ptr_ReadConsole -> Rp->ReadConsole -> GuiReadConsole -> ... -> wgl_histadd`
#
# We override `ptr_ReadConsole` directly on Unix and `Rp->ReadConsole` on
# Windows, and currently our `read_console()` method that we override with
# ignores `addtohistory`. We should probably implement our own history buffer at
# some point and actually respect this. For now, we don't have to worry about
# this path touching any C level history buffer because we control it.
#
# On Unix, going through `timestamp()` and GNU Readline's `add_history()` seems
# to work fine, even though its a little misleading because we currently never
# actually record anything else in the history buffer via `read_console()`, so
# the history buffer is either going to be empty or just a bunch of timestamps.
#
# On Windows, due to `wgl_hist_init()` not being called during startup, and us
# not having any way to do so, writing to the C history buffer via
# `wgl_histadd()` is not allowed, because it isn't ever initialized to allocated
# memory. Unfortunately, `timestamp()` will try and do this as shown by the path
# written out above, and, due to this bug in R, will try to write to the buffer
# even if it is uninitialized, resulting in a crash:
# https://bugs.r-project.org/show_bug.cgi?id=19064
#
# Since on Windows there is no equivalent to `ptr_R_addhistory`, i.e. there is
# no `Rp->addhistory` callback, the best we can do is override `timestamp()`
# directly with a version that doesn't try to write to the history buffer.
#
# # Future work
#
# Eventually we probably want to respect `R_ReadConsole(addtohistory = 1)` so we
# can write out `.Rhistory` files. To do this right, we should do all of these:
#
# - Create a Rust level `HistoryBuffer: Vec<String>`
# - Override R level `timestamp()` to write to `HistoryBuffer`
# - When `R_ReadConsole(addtohistory = 1)`, write `buf` to `HistoryBuffer`
# - Override R level `savehistory(file)` to write from `HistoryBuffer` into
#   `file` with `R_HistorySize` controlling the number of lines to write
# - Override R level `loadhistory(file)` to write from `file` into `HistoryBuffer`
# - Override `ptr_R_Cleanup` and `Rp->Cleanup` to write `HistoryBuffer` into
#   `R_HistoryFile` with size `R_HistorySize` on exit if `SA_SAVE` is set
# - On startup, if `R_RestoreHistory` is set then write from `R_HistoryFile`
#   into `HistoryBuffer`.
#
# And note that `R_setupHistory()` should be called before each access to
# `R_HistorySize` and `R_HistoryFile` to ensure that they have been updated from
# the dynamic environment variables `R_HISTSIZE` and `R_HISTFILE`, which R will
# do for us if we call that.
#
# On Unix we could override `ptr_R_loadhistory`, `ptr_R_savehistory`, and
# `ptr_R_addhistory` instead of the R level `loadhistory()`, `savehistory()`,
# and `timestamp()`, but since we can't do that on Windows, and since those
# pointers aren't used from anywhere else, we might as well just consistently
# override the R functions instead.

# Precisely `utils::timestamp()`, but without the `C_addhistory` call
windows_timestamp <- function(
    stamp = date(),
    prefix = "##------ ",
    suffix = " ------##",
    quiet = FALSE
) {
    stamp <- paste0(prefix, stamp, suffix)
    # .External2(C_addhistory, stamp)
    if (!quiet) {
        cat(stamp, sep = "\n")
    }
    invisible(stamp)
}
