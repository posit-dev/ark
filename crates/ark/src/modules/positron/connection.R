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
                actions = actions
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
.ps.connection_preview_object <- function(id, ..., table) {
    con <- get(id, getOption("connectionObserver")$.connections)
    if (is.null(con)) {
        return(NULL)
    }
    View(con$previewObject(table = table, ..., limit = 100), title = table)
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

    object_types <- con$listObjectTypes()
    object_types <- object_types[[1]] # root is always element 1

    for (p in path) {
        object_types <- object_types$contains[[p]]
    }

    object_types$icon
}
