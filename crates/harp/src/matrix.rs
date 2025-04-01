use libr::*;

use crate::r_dim;
use crate::r_int_get;
use crate::r_length;
use crate::utils::*;

/// Matrix support
///
/// # Notes
///
/// Currently we only utilize this for the static [Matrix::dim()] method,
/// but we could actually wrap matrices here (with validation) and provide
/// additional methods, like [harp::DataFrame].
pub struct Matrix {}

impl Matrix {
    /// Compute the dimensions of a matrix
    pub fn dim(x: SEXP) -> crate::Result<(i32, i32)> {
        if !r_is_matrix(x) {
            return Err(crate::anyhow!("`x` must be a matrix"));
        }

        let dim = r_dim(x);

        if r_typeof(dim) != INTSXP || r_length(dim) != 2 {
            return Err(crate::anyhow!(
                "`dim` must be an integer vector of length 2"
            ));
        }

        Ok((r_int_get(dim, 0), r_int_get(dim, 1)))
    }
}
