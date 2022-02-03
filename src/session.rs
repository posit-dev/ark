/*
 * session.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

#[derive(Clone)]
pub struct Session {
    pub hmac: Option<Hmac<Sha256>>,
    pub username: String,
    pub session_id: String,
}

impl Session {
    pub fn create(key: String) -> Result<Self, Error> {
        let hmac_key = match key.len() {
            0 => None,
            _ => {
                let result = match Hmac::<Sha256>::new_from_slice(key.as_bytes()) {
                    Ok(hmac) => hmac,
                    Err(err) => return Err(Error::HmacKeyInvalid(key, err)),
                };
                Some(result)
            }
        };
        Ok(Self {
            hmac: hmac_key,
            session_id: Uuid::new_v4().to_string(),
            username: String::from("kernel"),
        })
    }
}
