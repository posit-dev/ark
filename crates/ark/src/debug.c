typedef struct SEXPREC *SEXP;

// C entry points that are visible to debuggers like lldb.
// To ensure these functions are linked in, we pretend-call them from a
// placeholder in `utils.rs`.

const char* ark_print_rs(SEXP x);
const char* ark_display_value_rs(SEXP x);

const char* ark_print(SEXP x) {
    return ark_print_rs(x);
}

const char* ark_display_value(SEXP x) {
    return ark_display_value_rs(x);
}
