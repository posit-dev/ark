# Extending the Variables Pane in Ark

Ark allows package authors to customize how the variables pane displays specific R objects by defining custom methods, similar to S3 methods.

![Variables pane annotated](variables-pane.png)

## Defining Ark Methods for S3 Classes

To implement an Ark Variables Pane method for an S3 class (`"foo"`) in an R package, define pseudo-S3 methods like this:

```r
ark_variable_display_value.foo <- function(x, ..., width = NULL) {
    toString(x, width)
}
```

These methods don't need to be exported in the `NAMESPACE` file. Ark automatically finds and registers them when the package is loaded.

You can also register a method outside an R package using `.ps.register_ark_method()`, similar to `base::.S3method()`:

```r
.ps.register_ark_method("ark_variable_display_value", "foo",
                        function(x, width) { toString(x, width) })
```

## Available Methods

Ark currently supports six methods with the following signatures:

- `ark_variable_display_value(x, ..., width = getOption("width"))`
- `ark_variable_display_type(x, ..., include_length = TRUE)`
- `ark_variable_kind(x, ...)`
- `ark_variable_has_children(x, ...)`
- `ark_variable_get_children(x, ...)`
- `ark_variable_get_child_at(x, ..., index, name)`

### Customizing Display Value

The `ark_variable_display_value` method customizes how the display value of an object is shown. This is the text marked as "1. Display value" in the image above.

Example:

```r
#' @param x Object to get the display value for
#' @param width Maximum expected width. This is just a suggestion, the UI
#'   can stil truncate the string to different widths.
ark_variable_display_value.foo <- function(x, ..., width = getOption("width")) {
    "Hello world"  # Should return a length 1 character vector.
}
```

### Customizing Display Type

The `ark_variable_display_type` method customizes how the type of an object is shown. This is marked as "2. Display type" in the image.

Example:

```r
#' @param x Object to get the display type for
#' @param include_length Boolean indicating whether to include object length.
ark_variable_display_type.foo <- function(x, ..., include_length = TRUE) {
    sprintf("foo(%d)", length(x))
}
```

### Specifying Variable Kind

The `ark_variable_kind` method defines the kind of the variable. This allows the UI to organize variables in the variables pane differently. Currently, only `"table"` is used, but other possible values are [listed here](https://github.com/posit-dev/ark/blob/50f335183c5a13eda561a48d2ce21441caa79937/crates/amalthea/src/comm/variables_comm.rs#L107-L160).

Example:

```r
#' @param x Object to get the variable kind for
ark_variable_kind.foo <- function(x, ...) {
    "other"
}
```

## Inspecting Objects

Package authors can also implement methods that allow users to inspect R objects, similar to how the `str()` function works in R. This enables displaying object structures in the variables pane.

### Checking for Children

To check if an object has children that can be inspected, implement the `ark_variable_has_children` method:

```r
#' @param x Check if `x` has children
ark_variable_has_children.foo <- function(x, ...) {
    TRUE  # Return TRUE if the object can be inspected, FALSE otherwise.
}
```

### Getting Children and Child Elements

To allow inspection, implement these methods:

- `ark_variable_get_children`: Returns a named list of child objects to be displayed.
- `ark_variable_get_child_at`: Retrieves a specific element from the object.

Example:

```r
ark_variable_get_children.foo <- function(x, ...) {
    # Return an R list of children. The order of children should be
    # stable between repeated calls on the same object. For example:
    list(
        a = c(1, 2, 3),
        b = "Hello world",
        c = list(1, 2, 3)
    )
}

#' @param index An integer > 1, representing the index position of the child in the
#'   list returned by `ark_variable_get_children()`.
#' @param name The name of the child, corresponding to `names(ark_variable_get_children(x))[index]`.
#'   This may be a string or `NULL`. If using the name, it is the method author's responsibility to ensure
#'   the name is a valid, unique accessor. Additionally, if the original name from `ark_variable_get_children()`
#'   was too long, `ark` may discard the name and supply `name = NULL` instead.
ark_variable_get_child_at.foo <- function(x, ..., name, index) {
    # This could be implemented as:
    #   ark_variable_get_children(x)[[index]]
    # However, we expose an API that allows access by either name or index
    # without needing to rebuild the full list of children.

    if (name == "a") {
        c(1, 2, 3)
    } else if (name == "b") {
        "Hello world"
    } else if (name == "c") {
        list(1, 2, 3)
    } else {
        stop("Unknown name: ", name)
    }
}
```
