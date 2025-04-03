typedef enum {
    NILSXP	= 0,	/* nil = NULL */
    SYMSXP	= 1,	/* symbols */
    LISTSXP	= 2,	/* lists of dotted pairs */
    CLOSXP	= 3,	/* closures */
    ENVSXP	= 4,	/* environments */
    PROMSXP	= 5,	/* promises: [un]evaluated closure arguments */
    LANGSXP	= 6,	/* language constructs (special lists) */
    SPECIALSXP	= 7,	/* special forms */
    BUILTINSXP	= 8,	/* builtin non-special forms */
    CHARSXP	= 9,	/* "scalar" string type (internal only)*/
    LGLSXP	= 10,	/* logical vectors */
    INTSXP	= 13,	/* integer vectors */
    REALSXP	= 14,	/* real variables */
    CPLXSXP	= 15,	/* complex variables */
    STRSXP	= 16,	/* string vectors */
    DOTSXP	= 17,	/* dot-dot-dot object */
    ANYSXP	= 18,	/* make "any" args work */
    VECSXP	= 19,	/* generic vectors */
    EXPRSXP	= 20,	/* expressions vectors */
    BCODESXP	= 21,	/* byte code */
    EXTPTRSXP	= 22,	/* external pointer */
    WEAKREFSXP	= 23,	/* weak reference */
    RAWSXP	= 24,	/* raw bytes */
    OBJSXP	= 25,	/* S4 non-vector */

    NEWSXP      = 30,   /* fresh node created in new page */
    FREESXP     = 31,   /* node released by GC */

    FUNSXP	= 99	/* Closure or Builtin */
} SEXPTYPE;

typedef struct SEXPREC *SEXP;
