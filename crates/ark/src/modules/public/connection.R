#
# connection.R
#
# Copyright (C) 2023 Posit Software, PBC. All rights reserved.
#
#

.ps.connection_opened <- function(name) {
    .ps.Call("ps_connection_opened", name)
}

.ps.connection_closed <- function(id) {
    .ps.Call("ps_connection_closed", id)
}

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

.ps.connection_list_objects <- function(id, ...) {
    con <- get(id, getOption("connectionObserver")$.connections)
    if (is.null(con)) {
        return(data.frame(name = character(), type = character()))
    }
    con$listObjects(...)
}

.ps.connection_list_fields <- function(id, ...) {
    con <- get(id, getOption("connectionObserver")$.connections)
    if (is.null(con)) {
        return(data.frame(name = character(), type = character()))
    }
    con$listColumns(...)
}

.ps.connection_preview_object <- function(id, ..., table) {
    con <- get(id, getOption("connectionObserver")$.connections)
    if (is.null(con)) {
        return(NULL)
    }
    View(con$previewObject(table = table, ..., limit = 100), title = table)
}
