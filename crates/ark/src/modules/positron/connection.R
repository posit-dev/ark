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

            # check if the connection is already opened.
            # here's how this is done in RStudio:
            # https://github.com/rstudio/rstudio/blob/2344a0bf04657a13c36053eb04bcc47616a623dc/src/cpp/session/modules/SessionConnections.R#L48-L59ß
            for (id in ls(envir = connections)) {
                con <- get(id, envir = connections)
                if (identical(con$host, host) && identical(con$type, type)) {
                    return(invisible(id))
                }
            }

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
    # See link below for the original implementation
    # https://github.com/rstudio/rstudio/blob/fac89e1c4179fd23f47ff218bb106fd4e5cf2917/src/cpp/session/modules/SessionConnections.R#L165
    object_types <- list()
    while (length(object_tree) != 0) {
        object <- object_tree[[1]]
        name <- names(object_tree)[1]
        object_types[[name]] <- object

        object_tree <- object_tree[-1]
        if (!is.null(object$contains) && !identical(object$contains, "data")) {
            contains <- object$contains[!names(object$contains) %in% names(object_types)]
            object_tree <- c(contains, object_tree)
        }
    }
    object_types
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
    # we assume the first unnamed argument refers to the number of rows that
    # will be collected; this is basically what RStudio does here:
    # https://github.com/rstudio/rstudio/blob/018ea143118a15d46a5eaef16a43aef28ac03fb9/src/cpp/session/modules/connections/SessionConnections.cpp#L477-L480
    table <- con$previewObject(1000, ...)
    utils::View(table, title = utils::tail(path, 1)[[1]])
}

#' @export
.ps.connection_close <- function(id, ...) {
    con <- getOption("connectionObserver")$.connections[[id]]
    if (is.null(con)) {
         # this value is used to determine if we should send a close msg to the frontend
         # ie. closing the connection was an action from the R console, not from the frontend
        return(FALSE)
    }
    # disconnect is resposible for calling connectionClosed that
    # will remove the connection from the list of connections
    con$disconnect(...)
    return(TRUE)
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
    identical(object_types$contains, "data")
}

.ps.register_dummy_connection <- function() {
    # This is used for testing the connections service
    observer <- getOption("connectionObserver")
    id <- observer$connectionOpened(
        type = "DummyConnection",
        host = "DummyHost",
        displayName = "Dummy Connection",
        icon = "dummy-connection.png",
        connectCode = "print('Connected to Dummy Connection')",
        disconnect = function() print("Disconnected from Dummy Connection"),
        listObjectTypes = function() {
            list(
                schema = list(
                    icon = "schema.png",
                    contains = list(
                        table = list(
                            icon = "table.png",
                            contains = "data"
                        ),
                        view = list(
                            contains = "data"
                        )
                    )
                )
            )
        },
        listObjects = function(...) {
            path <- list(...)

            if (length(path) == 0) {
                return(data.frame(name = c("main"), type = c("schema")))
            }

            if (length(path) == 1) {
                if (path$schema == "main") {
                    return(data.frame(
                        name = c("table1", "table2", "view1"),
                        type = c("table", "table", "view")
                    ))
                }
            }

            stop("No more levels in the hierarchy")
        },
        listColumns = function(...) {
            path <- list(...)

            if (length(path) != 2) {
                stop("Need two levels in this path")
            }

            result <- data.frame(
                name = c("col1", "col2", "col3"),
                type = c("integer", "character", "logical")
            )

            result$name <- paste0(path[[2]], "_", result$name)
            result
        },
        previewObject = function(...) {
            data.frame(
                col1 = 1:10,
                col2 = letters[1:10],
                col3 = rep(c(TRUE, FALSE), 5)
            )
        },
        connectionObject = NULL,
        actions = NULL
    )

    if (is.null(id)) return("hello")
    id
}
