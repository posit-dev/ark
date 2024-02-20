#
# connection.R
#
# Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
#
#

#' @export
.ps.connection_opened <- function(name) {
    .ps.Call("ps_connection_opened", name)
}

#' @export
.ps.connection_closed <- function(id) {
    .ps.Call("ps_connection_closed", id)
}

#' @export
.ps.connection_observer <- function() {

    connections <- new.env(parent = emptyenv())

    connectionOpened <- function (type, host, displayName, icon = NULL,
        connectCode, disconnect, listObjectTypes, listObjects,
        listColumns, previewObject, connectionObject,
        actions = NULL) {
            id <- .ps.connection_opened(displayName)
            connections[[id]] <- list(
                type = type,
                host = host,
                displayName = displayName,
                icon = icon,
                connectCode = connectCode,
                disconnect = disconnect,
                listObjectTypes = listObjectTypes,
                listObjects = listObjects,
                listColumns = listColumns,
                previewObject = previewObject,
                connectionObject = connectionObject,
                actions = actions,
                # objectTypes are computed only once when creating the connection and are assumed to be static
                # until the end of the connection.
                objectTypes = connection_flatten_object_types(listObjectTypes())
            )
        invisible(id)
    }

    connectionClosed <- function(type, host) {
        for (id in names(connections)) {
            con <- connections[[id]]
            if (con$host == host && con$type == type) {
                .ps.connection_closed(id)
                rm(list = id, envir = connections)
                break
            }
        }
    }

    connectionUpdated <- function(type, host) {

    }


    list(
        connectionOpened = connectionOpened,
        connectionClosed = connectionClosed,
        connectionUpdated = connectionUpdated,
        .connections = connections
    )
}

options("connectionObserver" = .ps.connection_observer())

connection_flatten_object_types <- function(object_tree) {
    # RStudio actually flattens the objectTree to make it easier to find metadata for an object type.
    # See link below for the original implementation, which we copied here with small modifications.
    # https://github.com/rstudio/rstudio/blob/fac89e1c4179fd23f47ff218bb106fd4e5cf2917/src/cpp/session/modules/SessionConnections.R#L165
    # function to flatten the tree of object types for more convenient storage
    promote <- function(name, l) {

        if (length(l) == 0) return(list())

        if (is.null(l$contains) || identical(l$contains, "data")) {
            # plain data
            return(list(list(
                name = name,
                icon = l$icon,
                contains = "data"
            )))
        }

        # subtypes
        unlist(
            append(
                list(list(list(
                    name = name,
                    icon = l$icon,
                    contains = names(l$contains)
                ))),
                lapply(names(l$contains), function(name) {
                    promote(name, l$contains[[name]])
                })
            ),
            recursive = FALSE
        )
    }

    # apply tree flattener to provided object tree
    objectTypes <- lapply(names(object_tree), function(name) {
        promote(name, object_tree[[name]])
    })[[1]]
    names(objectTypes) <- sapply(objectTypes, function(x) x$name)
    objectTypes
}

#' @export
.ps.connection_list_objects <- function(id, ...) {
    con <- get(id, getOption("connectionObserver")$.connections)
    if (is.null(con)) {
        return(data.frame(name = character(), type = character()))
    }
    con$listObjects(...)
}

#' @export
.ps.connection_list_fields <- function(id, ...) {
    con <- get(id, getOption("connectionObserver")$.connections)
    if (is.null(con)) {
        return(data.frame(name = character(), type = character()))
    }
    con$listColumns(...)
}

#' @export
.ps.connection_preview_object <- function(id, ...) {
    path <- list(...)
    con <- get(id, getOption("connectionObserver")$.connections)
    if (is.null(con)) {
        return(NULL)
    }
    table <- con$previewObject(..., rowLimit = 1000)
    utils::View(table, title = utils::tail(path, 1)[[1]])
}

.ps.connection_close <- function(id, ...) {
    con <- get(id, getOption("connectionObserver")$.connections)
    if (is.null(con)) {
        return(NULL)
    }
    # disconnect is resposible for calling connectionClosed that
    # will remove the connection from the list of connections
    con$disconnect(...)
}

#' @export
.ps.connection_icon <- function(id, ...) {
    con <- get(id, getOption("connectionObserver")$.connections)
    path <- names(list(...))

    if (length(path) == 0) {
        # we are at the root of the connection
        return(con$icon)
    }

    object_types <- con$objectTypes[[utils::tail(path, 1)]]
    object_types$icon
}

#' @export
.ps.connection_contains_data <- function(id, ...) {
    con <- get(id, getOption("connectionObserver")$.connections)
    path <- names(list(...))

    if (length(path) == 0) {
        # we are at the root of the connection, so must not contain data.
        return(FALSE)
    }

    object_types <- con$objectTypes[[utils::tail(path, 1)]]
    identical(object$contains, "data")
}
