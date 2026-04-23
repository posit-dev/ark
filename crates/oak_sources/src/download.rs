use std::time::Duration;

const HTTP_NOT_FOUND: u16 = 404;
const HTTP_SERVICE_UNAVAILABLE: u16 = 503;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const GLOBAL_TIMEOUT: Duration = Duration::from_secs(40);

/// Outcome of a CRAN mirror HTTP request
pub(crate) enum Outcome {
    Success(ureq::http::Response<ureq::Body>),
    NotFound,
}

pub(crate) fn download_with_mirrors(suffix: &str, mirrors: &[&str]) -> anyhow::Result<Outcome> {
    if mirrors.is_empty() {
        panic!("`mirrors` can't be empty.");
    }

    let mut last_error = None;

    for mirror in mirrors {
        let url = format!("{mirror}/{suffix}");

        let request = ureq::get(&url)
            .config()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(GLOBAL_TIMEOUT))
            .build();

        match request.call() {
            Ok(response) => return Ok(Outcome::Success(response)),

            // Known to be not there, don't try any other mirrors
            Err(ureq::Error::StatusCode(HTTP_NOT_FOUND)) => return Ok(Outcome::NotFound),

            // Try next mirror, this one is temporarily unavailable
            Err(ureq::Error::StatusCode(HTTP_SERVICE_UNAVAILABLE)) => {
                last_error = Some(Err(ureq::Error::StatusCode(HTTP_SERVICE_UNAVAILABLE).into()));
                continue;
            },

            // Try next mirror, this one timed out
            Err(ureq::Error::Timeout(timeout)) => {
                last_error = Some(Err(ureq::Error::Timeout(timeout).into()));
                continue;
            },

            // Some unhandled error occurred, bail
            Err(err) => return Err(err.into()),
        };
    }

    // Every mirror returned `HTTP_SERVICE_UNAVAILABLE` or timed out
    last_error.expect("`mirrors` was non-empty and we always set `last_error`")
}
