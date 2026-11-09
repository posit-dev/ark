# Print the object names of installed packages' lazy data.
#
# Each `data/Rdata.rds` is a named list from dataset names to the names of the
# objects the dataset creates. `pkg::name` reaches objects, not datasets, so
# only the object names are printed.
#
# The single command-line argument names a UTF-8 file with one `Rdata.rds`
# path per line. Output is one `<index>\t<name>` line per object, where the
# index is the input path's zero-based line number. An unreadable index file
# produces a message on stderr and no names, without stopping other reads.
# Exit with code 1 unless exactly one argument is supplied.

args <- commandArgs(trailingOnly = TRUE)

if (length(args) != 1L) {
    message("'paths' is a required argument.")
    quit(status = 1L)
}

paths <- readLines(args[[1L]], encoding = "UTF-8", warn = FALSE)

for (i in seq_along(paths)) {
    path <- paths[[i]]

    names <- tryCatch(
        unlist(readRDS(path), use.names = FALSE),
        error = function(err) {
            message("Can't read '", path, "': ", conditionMessage(err))
            NULL
        }
    )
    if (!is.character(names)) {
        next
    }

    # Omit names that would be split into multiple output fields or records.
    names <- enc2utf8(names)
    names <- names[!grepl("[\t\r\n]", names)]

    if (length(names)) {
        writeLines(paste0(i - 1L, "\t", names), useBytes = TRUE)
    }
}
